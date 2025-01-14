use liner::Context;
use std::cell::RefCell;
use std::env;
use std::ffi::CStr;
use std::fs;
use std::fs::create_dir_all;
use std::io::{self, ErrorKind};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;

use nix::sys::signal::{self, SigHandler, Signal};
use nix::unistd::gethostname;

use crate::builtins_util::*;
use crate::completions::*;
use crate::environment::*;
use crate::process::*;
use crate::reader::*;
use crate::types::*;

fn call_lambda(
    environment: &mut Environment,
    lambda: &Lambda,
    args: &[Expression],
) -> io::Result<Expression> {
    // DO NOT use ? in here, need to make sure the new_scope is popped off the
    // current_scope list before ending.
    let mut looping = true;
    let mut last_eval = Ok(Expression::Atom(Atom::Nil));
    let mut new_scope = Scope::default();
    if let Err(err) = setup_args(
        environment,
        Some(&mut new_scope),
        &lambda.params,
        args,
        true,
    ) {
        return Err(err);
    }
    new_scope.outer = Some(lambda.capture.clone());
    environment
        .current_scope
        .push(Rc::new(RefCell::new(new_scope)));
    let old_loose = environment.loose_symbols;
    environment.loose_symbols = false;
    while looping {
        last_eval = eval(environment, &lambda.body);
        looping = environment.state.recur_num_args.is_some();
        if looping {
            let recur_args = environment.state.recur_num_args.unwrap();
            environment.state.recur_num_args = None;
            if let Ok(Expression::List(new_args)) = &last_eval {
                if recur_args != new_args.len() {
                    environment.current_scope.pop();
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "Called recur in a non-tail position.",
                    ));
                }
                if let Err(err) = setup_args(environment, None, &lambda.params, &new_args, false) {
                    environment.current_scope.pop();
                    return Err(err);
                }
            }
        }
    }
    environment.loose_symbols = old_loose;
    environment.current_scope.pop();
    last_eval
}

fn expand_macro(
    environment: &mut Environment,
    sh_macro: &Macro,
    args: &[Expression],
) -> io::Result<Expression> {
    // DO NOT use ? in here, need to make sure the new_scope is popped off the
    // current_scope list before ending.
    let mut new_scope = Scope::default();
    match setup_args(
        environment,
        Some(&mut new_scope),
        &sh_macro.params,
        args,
        false,
    ) {
        Ok(scope) => scope,
        Err(err) => return Err(err),
    };
    new_scope.outer = Some(environment.current_scope.last().unwrap().clone());
    environment
        .current_scope
        .push(Rc::new(RefCell::new(new_scope)));
    match eval(environment, &sh_macro.body) {
        Ok(expansion) => {
            environment.current_scope.pop();
            // Mess with eval_level to remove the extra level the macro added- helpful for executables and stdout detection.
            environment.state.eval_level -= 1;
            let result = eval(environment, &expansion);
            environment.state.eval_level += 1;
            result
        }
        Err(err) => {
            environment.current_scope.pop();
            Err(err)
        }
    }
}

fn internal_eval(environment: &mut Environment, expression: &Expression) -> io::Result<Expression> {
    let in_recur = environment.state.recur_num_args.is_some();
    if in_recur {
        environment.state.recur_num_args = None;
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
            match command {
                Expression::Atom(Atom::Symbol(_s)) => {}
                Expression::List(_list) => {}
                _ => {
                    let msg = format!(
                        "Not a valid command {}, must be a symbol.",
                        command.make_string(environment)?
                    );
                    return Err(io::Error::new(io::ErrorKind::Other, msg));
                }
            };

            match command {
                Expression::Atom(Atom::Symbol(command)) => {
                    if command.is_empty() {
                        return Ok(Expression::Atom(Atom::Nil));
                    }
                    let form = if environment.form_type == FormType::Any
                        || environment.form_type == FormType::FormOnly
                    {
                        get_expression(environment, &command)
                    } else {
                        None
                    };
                    if form.is_some() {
                        let exp = &*form.unwrap();
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
                    } else if environment.form_type == FormType::ExternalOnly
                        || environment.form_type == FormType::Any
                    {
                        do_command(environment, command, parts)
                    } else {
                        let msg = format!("Not a valid form {}, not found.", command.to_string());
                        Err(io::Error::new(io::ErrorKind::Other, msg))
                    }
                }
                Expression::List(list) => {
                    match eval(environment, &Expression::List(list.to_vec()))? {
                        Expression::Atom(Atom::Lambda(l)) => call_lambda(environment, &l, parts),
                        Expression::Atom(Atom::Macro(m)) => expand_macro(environment, &m, parts),
                        Expression::Func(f) => f(environment, &parts),
                        _ => Err(io::Error::new(io::ErrorKind::Other, "Not a valid command")),
                    }
                }
                Expression::Atom(Atom::Lambda(l)) => call_lambda(environment, &l, parts),
                Expression::Atom(Atom::Macro(m)) => expand_macro(environment, &m, parts),
                Expression::Func(f) => f(environment, &parts),
                _ => Err(io::Error::new(io::ErrorKind::Other, "Not a valid command")),
            }
        }
        Expression::Atom(Atom::Symbol(s)) => {
            if s.starts_with('$') {
                match env::var(&s[1..]) {
                    Ok(val) => Ok(Expression::Atom(Atom::String(val))),
                    Err(_) => Ok(Expression::Atom(Atom::String("".to_string()))),
                }
            } else if let Some(exp) = get_expression(environment, &s[..]) {
                if let Expression::Func(_) = *exp {
                    Ok(Expression::Atom(Atom::String(s.clone())))
                } else {
                    let exp = &*exp;
                    Ok(exp.clone())
                }
            } else if environment.loose_symbols {
                Ok(Expression::Atom(Atom::String(s.clone())))
            } else {
                let msg = format!("Symbol {} not found.", s);
                Err(io::Error::new(io::ErrorKind::Other, msg))
            }
        }
        Expression::Atom(atom) => Ok(Expression::Atom(atom.clone())),
        Expression::Func(_) => Ok(Expression::Atom(Atom::Nil)),
        Expression::Process(state) => Ok(Expression::Process(*state)),
    }
}

pub fn eval(environment: &mut Environment, expression: &Expression) -> io::Result<Expression> {
    environment.state.eval_level += 1;
    let result = internal_eval(environment, expression);
    if let Err(_err) = &result {
        eprintln!(
            "{}: Error evaluting {}",
            environment.state.eval_level,
            expression.to_string()
        );
    }
    environment.state.eval_level -= 1;
    result
}

fn load_scripts(environment: &mut Environment, home: &str) {
    let mut script = format!("{}/.config/slsh/slsh_std.lisp", home);
    if let Err(err) = run_script(&script, environment) {
        eprintln!(
            "WARNING: Failed to load standard macros script {}: {}",
            script, err
        );
    }
    script = format!("{}/.config/slsh/slsh_shell.lisp", home);
    if let Err(err) = run_script(&script, environment) {
        eprintln!(
            "WARNING: Failed to load shell macros script {}: {}",
            script, err
        );
    }
    script = format!("{}/.config/slsh/slshrc", home);
    if let Err(err) = run_script(&script, environment) {
        eprintln!("WARNING: Failed to load init script {}: {}", script, err);
    }
}

fn get_prompt(environment: &mut Environment) -> String {
    if environment
        .root_scope
        .borrow()
        .data
        .contains_key("__prompt")
    {
        let mut exp = environment
            .root_scope
            .borrow()
            .data
            .get("__prompt")
            .unwrap()
            .clone();
        exp = match *exp {
            Expression::Atom(Atom::Lambda(_)) => {
                let mut v = Vec::with_capacity(1);
                v.push(Expression::Atom(Atom::Symbol("__prompt".to_string())));
                Rc::new(Expression::List(v))
            }
            _ => exp,
        };
        environment.save_exit_status = false; // Do not overwrite last exit status with prompt commands.
        let res = eval(environment, &exp);
        environment.save_exit_status = true;
        res.unwrap_or_else(|e| Expression::Atom(Atom::String(format!("ERROR: {}", e).to_string())))
            .make_string(environment)
            .unwrap_or_else(|_| "ERROR".to_string())
    } else {
        // Nothing set, use a default.
        let hostname = match env::var("HOST") {
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
        format!(
            "\x1b[32m{}:\x1b[34m{}\x1b[37m(slsh)\x1b[32m>\x1b[39m ",
            hostname,
            pwd.display()
        )
    }
}

pub fn start_interactive() {
    let mut con = Context::new();
    con.history.append_duplicate_entries = false;
    con.history.inc_append = true;
    con.history.load_duplicates = false;
    con.history.share = true;
    con.key_bindings = liner::KeyBindings::Vi;
    // Initialize the HOST variable
    let mut hostname = [0_u8; 512];
    env::set_var(
        "HOST",
        &gethostname(&mut hostname)
            .ok()
            .map_or_else(|| "?".into(), CStr::to_string_lossy)
            .as_ref(),
    );
    let mut home = match env::var("HOME") {
        Ok(val) => val,
        Err(_) => ".".to_string(),
    };
    if home.ends_with('/') {
        home = home[..home.len() - 1].to_string();
    }
    let share_dir = format!("{}/.local/share/slsh", home);
    if let Err(err) = create_dir_all(&share_dir) {
        eprintln!(
            "WARNING: Unable to create share directory: {}- {}",
            share_dir, err
        );
    }
    if let Err(err) = con
        .history
        .set_file_name_and_load_history(format!("{}/history", share_dir))
    {
        eprintln!("WARNING: Unable to load history: {}", err);
    }
    let environment = Rc::new(RefCell::new(build_default_environment()));
    load_scripts(&mut environment.borrow_mut(), &home);
    environment
        .borrow_mut()
        .root_scope
        .borrow_mut()
        .data
        .insert(
            "*last-status*".to_string(),
            Rc::new(Expression::Atom(Atom::Int(0))),
        );
    loop {
        environment.borrow_mut().state.stdout_status = None;
        environment.borrow_mut().state.stderr_status = None;
        let prompt = get_prompt(&mut environment.borrow_mut());
        if let Err(err) = reap_procs(&environment.borrow()) {
            eprintln!("Error reaping processes: {}", err);
        }
        let mut shell_completer = ShellCompleter::new(environment.clone());
        match con.read_line(prompt, None, &mut shell_completer) {
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
                // Clear the last status once something new is entered.
                env::set_var("LAST_STATUS".to_string(), format!("{}", 0));
                environment
                    .borrow_mut()
                    .root_scope
                    .borrow_mut()
                    .data
                    .insert(
                        "*last-status*".to_string(),
                        Rc::new(Expression::Atom(Atom::Int(i64::from(0)))),
                    );
                let ast = read(&mod_input);
                match ast {
                    Ok(ast) => {
                        environment.borrow_mut().loose_symbols = true;
                        let res = eval(&mut environment.borrow_mut(), &ast);
                        match res {
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
                                        if let Err(err) = exp.write(&environment.borrow()) {
                                            eprintln!("Error writing result: {}", err);
                                        }
                                    }
                                }
                            }
                            Err(err) => eprintln!("{}", err),
                        }
                        environment.borrow_mut().loose_symbols = false;
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

pub fn read_stdin() {
    let mut home = match env::var("HOME") {
        Ok(val) => val,
        Err(_) => ".".to_string(),
    };
    if home.ends_with('/') {
        home = home[..home.len() - 1].to_string();
    }
    let share_dir = format!("{}/.local/share/slsh", home);
    if let Err(err) = create_dir_all(&share_dir) {
        eprintln!(
            "WARNING: Unable to create share directory: {}- {}",
            share_dir, err
        );
    }
    let mut environment = build_default_environment();
    environment.is_tty = false;
    load_scripts(&mut environment, &home);

    let mut input = String::new();
    loop {
        match io::stdin().read_line(&mut input) {
            Ok(0) => return,
            Ok(_n) => {
                environment.state.stdout_status = None;
                let mod_input = if input.starts_with('(')
                    || input.starts_with('\'')
                    || input.starts_with('`')
                {
                    input.clone()
                } else {
                    format!("({})", input)
                };
                let ast = read(&mod_input);
                match ast {
                    Ok(ast) => {
                        environment.loose_symbols = true;
                        match eval(&mut environment, &ast) {
                            Ok(exp) => {
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
                        environment.loose_symbols = false;
                    }
                    Err(err) => eprintln!("{:?}", err),
                }
                environment.state.stderr_status = None;
            }
            Err(error) => {
                eprintln!("ERROR reading stdin: {}", error);
                return;
            }
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
    let ast = read(&contents);
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
        .root_scope
        .borrow_mut()
        .data
        .insert("args".to_string(), Rc::new(Expression::List(exp_args)));
    run_script(command, &mut environment)
}
