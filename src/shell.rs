use liner::Context;
use std::env;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use nix::sys::signal::{self, SigHandler, Signal};

use crate::builtins_util::*;
use crate::completions::*;
use crate::environment::*;
use crate::process::*;
use crate::script::*;
use crate::types::*;

fn call_lambda(
    environment: &mut Environment,
    lambda: &Lambda,
    args: &[Expression],
) -> io::Result<Expression> {
    let mut new_environment = build_new_scope(environment);
    let mut looping = true;
    let mut last_eval = Ok(Expression::Atom(Atom::Nil));
    setup_args(&mut new_environment, &lambda.params, args, true)?;
    while looping {
        last_eval = eval(&mut new_environment, &lambda.body);
        looping = environment.state.borrow().recur_num_args.is_some();
        if looping {
            let recur_args = environment.state.borrow().recur_num_args.unwrap();
            environment.state.borrow_mut().recur_num_args = None;
            if let Ok(Expression::List(new_args)) = &last_eval {
                if recur_args != new_args.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "Called recur in a non-tail position.",
                    ));
                }
                setup_args(&mut new_environment, &lambda.params, &new_args, false)?;
            }
        }
    }
    last_eval
}

fn expand_macro(
    environment: &mut Environment,
    sh_macro: &Lambda,
    args: &[Expression],
) -> io::Result<Expression> {
    let mut new_environment = build_new_scope(environment);
    setup_args(&mut new_environment, &sh_macro.params, args, false)?;
    let expansion = eval(&mut new_environment, &sh_macro.body)?;
    eval(environment, &expansion)
}

fn internal_eval(
    environment: &mut Environment,
    expression: &Expression,
    data_in: Option<Expression>,
) -> io::Result<Expression> {
    let in_recur = environment.state.borrow().recur_num_args.is_some();
    if in_recur {
        environment.state.borrow_mut().recur_num_args = None;
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "Called recur in a non-tail position.",
        ));
    }
    match expression {
        Expression::List(parts) => {
            let (command, parts) = match parts.split_first() {
                Some((c, p)) => (c, p),
                None => {
                    eprintln!("No valid command.");
                    return Err(io::Error::new(io::ErrorKind::Other, "No valid command."));
                }
            };
            let command = match command {
                Expression::Atom(Atom::Symbol(s)) => s,
                _ => {
                    let msg = format!(
                        "Not a valid command {}, must be a symbol.",
                        command.make_string(environment)?
                    );
                    return Err(io::Error::new(io::ErrorKind::Other, msg));
                }
            };
            if command.is_empty() {
                return Ok(Expression::Atom(Atom::Nil));
            }

            if let Some(exp) = get_expression(environment, &command) {
                if let Expression::Func(f) = exp {
                    f(environment, &parts)
                } else if let Expression::Atom(Atom::Lambda(f)) = exp {
                    call_lambda(environment, &f, parts)
                } else if let Expression::Atom(Atom::Macro(m)) = exp {
                    expand_macro(environment, &m, parts)
                } else {
                    let exp = exp.clone();
                    eval(environment, &exp)
                }
            } else {
                match &command[..] {
                    "nil" => Ok(Expression::Atom(Atom::Nil)),
                    "|" | "pipe" => do_pipe(environment, parts, data_in),
                    //"exit" => return,
                    command => do_command(environment, command, parts, data_in),
                }
            }
        }
        Expression::Atom(Atom::Symbol(s)) => {
            if s.starts_with('$') {
                match env::var(&s[1..]) {
                    Ok(val) => Ok(Expression::Atom(Atom::String(val))),
                    Err(_) => Ok(Expression::Atom(Atom::String("".to_string()))),
                }
            } else if let Some(exp) = get_expression(environment, &s[..]) {
                if let Expression::Func(_) = exp {
                    Ok(Expression::Atom(Atom::String(s.clone())))
                } else {
                    Ok(exp)
                }
            } else {
                Ok(Expression::Atom(Atom::String(s.clone())))
            }
        }
        Expression::Atom(atom) => Ok(Expression::Atom(atom.clone())),
        Expression::Func(_) => Ok(Expression::Atom(Atom::Nil)),
        Expression::Process(pid) => Ok(Expression::Process(*pid)), //Ok(Expression::Atom(Atom::Int(i64::from(*pid)))),
    }
}

pub fn pipe_eval(
    environment: &mut Environment,
    expression: &Expression,
    data_in: Option<Expression>,
) -> io::Result<Expression> {
    environment.state.borrow_mut().eval_level += 1;
    let result = internal_eval(environment, expression, data_in);
    environment.state.borrow_mut().eval_level -= 1;
    result
}

pub fn eval(environment: &mut Environment, expression: &Expression) -> io::Result<Expression> {
    pipe_eval(environment, expression, None)
}

pub fn start_interactive() {
    let mut con = Context::new();
    con.history.append_duplicate_entries = false;
    con.history.inc_append = true;
    con.history.load_duplicates = false;
    con.key_bindings = liner::KeyBindings::Vi;
    if let Err(err) = con.history.set_file_name_and_load_history("tmp_history") {
        eprintln!("Error loading history: {}", err);
    }
    let mut environment = build_default_environment();
    let home = match env::var("HOME") {
        Ok(val) => val,
        Err(_) => ".".to_string(),
    };
    let init_script = format!("{}/.config/slsh/slshrc", home);
    if let Err(err) = run_script(&init_script, &mut environment) {
        eprintln!("Failed to run init script {}: {}", init_script, err);
    }

    loop {
        let hostname = match env::var("HOSTNAME") {
            Ok(val) => val,
            Err(_) => "UNKNOWN".to_string(),
        };
        let pwd = match env::current_dir() {
            Ok(val) => val,
            Err(_) => {
                let mut p = PathBuf::new();
                p.push("/");
                p
            }
        };
        environment.state.borrow_mut().stdout_status = None;
        environment.state.borrow_mut().stderr_status = None;
        let prompt = if environment.data.contains_key("__prompt") {
            let mut exp = environment.data.get("__prompt").unwrap().clone();
            exp = match exp {
                Expression::Atom(Atom::Lambda(_)) => {
                    let mut v = Vec::with_capacity(1);
                    v.push(Expression::Atom(Atom::Symbol("__prompt".to_string())));
                    Expression::List(v)
                }
                _ => exp,
            };
            let res = eval(&mut environment, &exp);
            res.unwrap_or_else(|e| {
                Expression::Atom(Atom::String(format!("ERROR: {}", e).to_string()))
            })
            .make_string(&environment)
            .unwrap_or_else(|_| "ERROR".to_string())
        } else {
            format!(
                "\x1b[32m{}:\x1b[34m{}\x1b[37m(slsh)\x1b[32m>\x1b[39m ",
                hostname,
                pwd.display()
            )
        };
        let prompt = prompt.replace("\\x1b", "\x1b"); // Patch escape codes.
        if let Err(err) = reap_procs(&environment) {
            eprintln!("Error reaping processes: {}", err);
        }
        match con.read_line(prompt, None, &mut ShellCompleter) {
            Ok(input) => {
                if input.is_empty() {
                    continue;
                }
                let mod_input = if input.starts_with('(')
                    || input.starts_with('\'')
                    || input.starts_with('`')
                {
                    input.clone()
                } else {
                    format!("({})", input)
                };
                let tokens = tokenize(&mod_input);
                let ast = parse(&tokens);
                //println!("{:?}", ast);
                match ast {
                    Ok(ast) => {
                        match eval(&mut environment, &ast) {
                            Ok(exp) => {
                                if !input.is_empty() {
                                    if let Err(err) = con.history.push(input.into()) {
                                        eprintln!("Error saving history: {}", err);
                                    }
                                }
                                match exp {
                                    Expression::Atom(Atom::Nil) => { /* don't print nil */ }
                                    Expression::Process(_) => { /* should have used stdout */ }
                                    _ => {
                                        if let Err(err) = exp.write(&environment) {
                                            eprintln!("Error writing result: {}", err);
                                        }
                                    }
                                }
                            }
                            Err(err) => eprintln!("{}", err),
                        }
                    }
                    Err(err) => eprintln!("{:?}", err),
                }
            }
            Err(err) => match err.kind() {
                ErrorKind::UnexpectedEof => return,
                ErrorKind::Interrupted => {}
                _ => println!("Error on input: {}", err),
            },
        }
    }
}

fn parse_one_run_command_line(input: &str, nargs: &mut Vec<String>) -> io::Result<()> {
    let mut in_string = false;
    let mut in_stringd = false;
    let mut token = String::new();
    let mut last_ch = ' ';
    for ch in input.chars() {
        if ch == '\'' && last_ch != '\\' {
            // Kakoune bug "
            in_string = !in_string;
            if !in_string {
                nargs.push(token);
                token = String::new();
            }
            last_ch = ch;
            continue;
        }
        if ch == '"' && last_ch != '\\' {
            // Kakoune bug "
            in_stringd = !in_stringd;
            if !in_stringd {
                nargs.push(token);
                token = String::new();
            }
            last_ch = ch;
            continue;
        }
        if in_string || in_stringd {
            token.push(ch);
        } else if ch == ' ' {
            if !token.is_empty() {
                nargs.push(token);
                token = String::new();
            }
        } else {
            token.push(ch);
        }
        last_ch = ch;
    }
    if !token.is_empty() {
        nargs.push(token);
    }
    Ok(())
}

pub fn run_one_command(command: &str, args: &[String]) -> io::Result<()> {
    // Try to make sense out of whatever crap we get (looking at you fzf-tmux)
    // and make it work.
    let mut nargs: Vec<String> = Vec::new();
    parse_one_run_command_line(command, &mut nargs)?;
    for arg in args {
        parse_one_run_command_line(&arg, &mut nargs)?;
    }

    if !nargs.is_empty() {
        let mut com = Command::new(&nargs[0]); //command);
        if nargs.len() > 1 {
            com.args(&nargs[1..]);
        }
        com.stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdin(Stdio::inherit());

        unsafe {
            com.pre_exec(|| -> io::Result<()> {
                signal::signal(Signal::SIGINT, SigHandler::SigDfl).unwrap();
                signal::signal(Signal::SIGHUP, SigHandler::SigDfl).unwrap();
                signal::signal(Signal::SIGTERM, SigHandler::SigDfl).unwrap();
                Ok(())
            });
        }

        let mut proc = com.spawn()?;
        proc.wait()?;
    }
    Ok(())
}

fn run_script(file_name: &str, environment: &mut Environment) -> io::Result<()> {
    let contents = fs::read_to_string(file_name)?;
    let tokens = tokenize(&contents);
    let ast = parse(&tokens);
    match ast {
        Ok(Expression::List(list)) => {
            for exp in list {
                match eval(environment, &exp) {
                    Ok(_exp) => {}
                    Err(err) => {
                        eprintln!("{}", err);
                        return Err(err);
                    }
                }
            }
            Ok(())
        }
        Ok(ast) => match eval(environment, &ast) {
            Ok(_exp) => Ok(()),
            Err(err) => {
                eprintln!("{}", err);
                Err(err)
            }
        },
        Err(err) => {
            eprintln!("{:?}", err);
            Err(io::Error::new(io::ErrorKind::Other, err.reason))
        }
    }
}

pub fn run_one_script(command: &str, args: &[String]) -> io::Result<()> {
    let mut environment = build_default_environment();
    let mut exp_args: Vec<Expression> = Vec::with_capacity(args.len());
    for a in args {
        exp_args.push(Expression::Atom(Atom::String(a.clone())));
    }
    environment
        .data
        .insert("args".to_string(), Expression::List(exp_args));
    run_script(command, &mut environment)
}
