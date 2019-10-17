use std::env;
use std::fs::File;
use std::io::{self, Write};
use std::iter::FromIterator;
use std::rc::Rc;

use crate::environment::*;
use crate::eval::*;
use crate::types::*;

pub fn to_args(env: &mut Environment, parts: &[Expression]) -> io::Result<Vec<Expression>> {
    let mut args: Vec<Expression> = Vec::with_capacity(parts.len());
    for a in parts {
        args.push(eval(env, a)?);
    }
    Ok(args)
}

pub fn to_args_str(environment: &mut Environment, parts: &[Expression]) -> io::Result<Vec<String>> {
    let mut args: Vec<String> = Vec::with_capacity(parts.len());
    for a in parts {
        args.push(eval(environment, a)?.make_string(environment)?);
    }
    Ok(args)
}

fn args_out(
    environment: &mut Environment,
    args: &[Expression],
    add_newline: bool,
    writer: &mut dyn Write,
) -> io::Result<()> {
    for a in args {
        a.writef(environment, writer)?;
    }
    if add_newline {
        writer.write_all("\n".as_bytes())?;
    }
    Ok(())
}

fn print_to_oe(
    environment: &mut Environment,
    args: &[Expression],
    add_newline: bool,
    default_error: bool,
    key: &str,
) -> io::Result<()> {
    let out = get_expression(environment, key);
    match out {
        Some(out) => {
            if let Expression::File(f) = &*out {
                match f {
                    FileState::Stdout => {
                        let stdout = io::stdout();
                        let mut out = stdout.lock();
                        args_out(environment, args, add_newline, &mut out)?;
                    }
                    FileState::Stderr => {
                        let stdout = io::stderr();
                        let mut out = stdout.lock();
                        args_out(environment, args, add_newline, &mut out)?;
                    }
                    FileState::Write(f) => {
                        args_out(environment, args, add_newline, &mut *f.borrow_mut())?;
                    }
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            "ERROR: Can not print to a non-writable file.",
                        ));
                    }
                }
            } else {
                let msg = format!("ERROR: {} is not a file!", key);
                return Err(io::Error::new(io::ErrorKind::Other, msg));
            }
        }
        None => {
            if default_error {
                let stdout = io::stderr();
                let mut out = stdout.lock();
                args_out(environment, args, add_newline, &mut out)?;
            } else {
                let stdout = io::stdout();
                let mut out = stdout.lock();
                args_out(environment, args, add_newline, &mut out)?;
            }
        }
    }
    Ok(())
}

pub fn print(
    environment: &mut Environment,
    args: &[Expression],
    add_newline: bool,
) -> io::Result<Expression> {
    match &environment.state.stdout_status {
        Some(IOState::FileAppend(f)) => {
            let mut out = std::fs::OpenOptions::new()
                .read(false)
                .write(true)
                .append(true)
                .create(true)
                .open(&f)?;
            args_out(environment, args, add_newline, &mut out)?;
        }
        Some(IOState::FileOverwrite(f)) => {
            let mut out = File::create(f)?;
            args_out(environment, args, add_newline, &mut out)?;
        }
        Some(IOState::Null) => { /* Nothing to do... */ }
        _ => {
            print_to_oe(environment, args, add_newline, false, "*stdout*")?;
        }
    };
    Ok(Expression::Atom(Atom::Nil))
}

pub fn eprint(
    environment: &mut Environment,
    args: &[Expression],
    add_newline: bool,
) -> io::Result<Expression> {
    match &environment.state.stderr_status {
        Some(IOState::FileAppend(f)) => {
            let mut out = std::fs::OpenOptions::new()
                .read(false)
                .write(true)
                .append(true)
                .create(true)
                .open(&f)?;
            args_out(environment, args, add_newline, &mut out)?;
        }
        Some(IOState::FileOverwrite(f)) => {
            let mut out = File::create(f)?;
            args_out(environment, args, add_newline, &mut out)?;
        }
        Some(IOState::Null) => { /* Nothing to do... */ }
        _ => {
            print_to_oe(environment, args, add_newline, true, "*stderr*")?;
        }
    };
    Ok(Expression::Atom(Atom::Nil))
}

pub fn parse_list_of_ints(
    environment: &mut Environment,
    args: &mut [Expression],
) -> io::Result<Vec<i64>> {
    let mut list: Vec<i64> = Vec::with_capacity(args.len());
    for arg in args {
        list.push(arg.make_int(environment)?);
    }
    Ok(list)
}

pub fn parse_list_of_floats(
    environment: &mut Environment,
    args: &mut [Expression],
) -> io::Result<Vec<f64>> {
    let mut list: Vec<f64> = Vec::with_capacity(args.len());
    for arg in args {
        list.push(arg.make_float(environment)?);
    }
    Ok(list)
}

pub fn parse_list_of_strings(
    environment: &mut Environment,
    args: &mut [Expression],
) -> io::Result<Vec<String>> {
    let mut list: Vec<String> = Vec::with_capacity(args.len());
    for arg in args {
        list.push(arg.make_string(environment)?);
    }
    Ok(list)
}

pub fn expand_tilde(path: &str) -> Option<String> {
    let mut home = match env::var("HOME") {
        Ok(val) => val,
        Err(_) => "/".to_string(),
    };
    if !home.ends_with('/') {
        home.push('/');
    }
    if path == "~" {
        Some(home)
    } else if path.contains("~/") {
        let new_path = path.replace("~/", &home);
        Some(new_path)
    } else {
        None
    }
}

fn setup_args_final(
    scope: &mut Scope,
    var_names: &mut Vec<String>,
    vars: &[Expression],
    min_params: usize,
    use_rest: bool,
) -> io::Result<()> {
    if vars.len() < min_params || (!use_rest && vars.len() > min_params) {
        let msg = format!(
            "wrong number of parameters, expected {} got {}",
            min_params,
            vars.len()
        );
        return Err(io::Error::new(io::ErrorKind::Other, msg));
    } else if use_rest {
        let rest_name = var_names.pop().unwrap();
        for (k, v) in var_names.iter().zip(vars.iter()) {
            scope.data.insert(k.clone(), Rc::new(v.clone()));
        }
        if vars.len() > min_params {
            let rest_data: Vec<Expression> = Vec::from_iter(vars[min_params..].iter().cloned());
            scope
                .data
                .insert(rest_name.clone(), Rc::new(Expression::with_list(rest_data)));
        } else {
            scope
                .data
                .insert(rest_name.clone(), Rc::new(Expression::Atom(Atom::Nil)));
        }
    } else {
        for (k, v) in var_names.iter().zip(vars.iter()) {
            scope.data.insert(k.clone(), Rc::new(v.clone()));
        }
    }
    Ok(())
}

pub fn setup_args(
    environment: &mut Environment,
    new_scope: Option<&mut Scope>,
    params: &Expression,
    args: &[Expression],
    eval_args: bool,
) -> io::Result<()> {
    if let Expression::List(l) = params {
        let l = l.borrow();
        let mut var_names: Vec<String> = Vec::with_capacity(l.len());
        let mut use_rest = false;
        let mut post_rest_cnt = 0;
        let mut min_params = 0;
        for arg in l.iter() {
            if let Expression::Atom(Atom::Symbol(s)) = arg {
                match &s[..] {
                    "&rest" => {
                        if use_rest {
                            return Err(io::Error::new(
                                io::ErrorKind::Other,
                                "&rest can only appear once",
                            ));
                        }
                        use_rest = true;
                    }
                    _ => {
                        if post_rest_cnt > 1 {
                            return Err(io::Error::new(
                                io::ErrorKind::Other,
                                "&rest can only have one symbol after",
                            ));
                        }
                        if use_rest {
                            post_rest_cnt += 1;
                        } else {
                            min_params += 1;
                        }
                        var_names.push(s.clone());
                    }
                }
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "parameter name must be symbol",
                ));
            }
        }
        if use_rest && post_rest_cnt != 1 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "&rest must have one symbol after",
            ));
        }
        let t_vars = if eval_args {
            Some(to_args(environment, args)?)
        } else {
            None
        };
        let last_scope = &mut environment.current_scope.last().unwrap().borrow_mut();
        let new_scope = match new_scope {
            Some(new_scope) => new_scope,
            None => last_scope,
        };
        if let Some(t_vars) = t_vars {
            setup_args_final(new_scope, &mut var_names, &t_vars, min_params, use_rest)?;
        } else {
            setup_args_final(new_scope, &mut var_names, args, min_params, use_rest)?;
        };
    }
    Ok(())
}
