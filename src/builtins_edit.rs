use std::collections::HashMap;
use std::env;
use std::hash::BuildHasher;
use std::io::{self, ErrorKind};

use liner::{ColorClosure, Context, Prompt};

use crate::builtins_util::*;
use crate::completions::*;
use crate::environment::*;
use crate::eval::*;
use crate::interner::*;
use crate::shell::apply_repl_settings;
use crate::shell::get_liner_words;
use crate::shell::load_repl_settings;
use crate::types::*;

fn make_con(environment: &mut Environment, history: Option<&str>) -> Context {
    let mut con = Context::new();
    con.set_word_divider(Box::new(get_liner_words));
    let mut home = match env::var("HOME") {
        Ok(val) => val,
        Err(_) => ".".to_string(),
    };
    if home.ends_with('/') {
        home = home[..home.len() - 1].to_string();
    }
    if let Some(history) = history {
        let history_file = if history.starts_with('/') || history.starts_with('.') {
            history.to_string()
        } else {
            format!("{}/.local/share/sl-sh/{}", home, history)
        };
        if let Err(err) = con.history.set_file_name_and_load_history(&history_file) {
            eprintln!(
                "WARNING: Unable to load history file {}: {}",
                history_file, err
            );
        }
    }
    apply_repl_settings(&mut con, &environment.repl_settings);
    con
}

fn get_color_closure(environment: &mut Environment) -> Option<ColorClosure> {
    let line_exp = get_expression(environment, "__line_handler");
    if let Some(exp) = line_exp {
        let exp = exp.exp;
        // This unsafe should be OK because the returned object is used in a call to read_line and
        // dropped after.
        let environment = unsafe { &mut *(environment as *mut Environment) };
        Some(Box::new(move |input: &str| -> String {
            let exp = match &exp.get().data {
                ExpEnum::Atom(Atom::Lambda(_)) => {
                    let mut v = Vec::with_capacity(1);
                    let sym = environment.interner.intern("__line_handler");
                    v.push(
                        Expression::alloc_data(ExpEnum::Atom(Atom::Symbol(sym))).handle_no_root(),
                    );
                    v.push(
                        Expression::alloc_data(ExpEnum::Atom(Atom::String(
                            input.to_string().into(),
                            None,
                        )))
                        .handle_no_root(),
                    );
                    Expression::with_list(v)
                }
                _ => return input.to_string(),
            };
            environment.save_exit_status = false; // Do not overwrite last exit status with line_handler.
            environment.str_ignore_expand = true;
            let res = eval(environment, exp);
            environment.str_ignore_expand = false;
            environment.save_exit_status = true;
            res.unwrap_or_else(|e| {
                Expression::alloc_data(ExpEnum::Atom(Atom::String(
                    format!("ERROR: {}", e).into(),
                    None,
                )))
            })
            .as_string(environment)
            .unwrap_or_else(|_| "ERROR".to_string())
        }))
    } else {
        None
    }
}

pub fn read_prompt(
    environment: &mut Environment,
    prompt: &str,
    history: Option<&str>,
    liner_id: &'static str,
) -> io::Result<String> {
    let repl_settings = get_expression(environment, "*repl-settings*").unwrap();
    let new_repl_settings = load_repl_settings(&repl_settings.exp);
    let mut load_settings = if environment.repl_settings != new_repl_settings {
        environment.repl_settings = new_repl_settings.clone();
        true
    } else {
        false
    };
    let mut con = if liner_id == ":new" {
        load_settings = false;
        make_con(environment, history)
    } else if environment.liners.contains_key(liner_id) {
        environment.liners.remove(liner_id).unwrap()
    } else {
        load_settings = false;
        make_con(environment, history)
    };
    if load_settings {
        apply_repl_settings(&mut con, &new_repl_settings);
    };
    // This unsafe should be OK because the con object this is set into is
    // stored in the environment (or dropped at the end of this function)
    // so environment should out live con.
    let env = unsafe { &mut *(environment as *mut Environment) };
    con.set_completer(Box::new(ShellCompleter::new(env)));
    let result = match con.read_line(Prompt::from(prompt), get_color_closure(environment)) {
        Ok(input) => {
            let input = input.trim();
            /*if history.is_some() {
                if let Err(err) = con.history.push(input) {
                    eprintln!("read-line: Error saving history: {}", err);
                }
            }*/
            Ok(input.into())
        }
        Err(err) => Err(err),
    };
    if liner_id != ":new" {
        environment.liners.insert(liner_id, con);
    };
    result
}

fn builtin_prompt(
    environment: &mut Environment,
    args: &mut dyn Iterator<Item = Expression>,
) -> Result<Expression, LispError> {
    let (liner_id, prompt) = {
        let arg1 = param_eval(environment, args, "prompt")?;
        let arg_d = arg1.get();
        if let ExpEnum::Atom(Atom::Symbol(s)) = arg_d.data {
            (s, param_eval(environment, args, "prompt")?)
        } else {
            drop(arg_d);
            (":new", arg1)
        }
    };
    let h_str;
    let history_file = if let Some(h) = args.next() {
        let hist = eval(environment, h)?;
        let hist_d = hist.get();
        if let ExpEnum::Atom(Atom::String(s, _)) = &hist_d.data {
            h_str = match expand_tilde(s) {
                Some(p) => p,
                None => s.to_string(),
            };
            Some(&h_str[..])
        } else {
            return Err(LispError::new(
                "prompt: history file (if provided) must be a string.",
            ));
        }
    } else {
        None
    };
    params_done(args, "prompt")?;
    let prompt_d = prompt.get();
    if let ExpEnum::Atom(Atom::String(s, _)) = &prompt_d.data {
        return match read_prompt(environment, s, history_file, liner_id) {
            Ok(input) => Ok(Expression::alloc_data(ExpEnum::Atom(Atom::String(
                input.into(),
                None,
            )))),
            Err(err) => match err.kind() {
                ErrorKind::UnexpectedEof => {
                    let input =
                        Expression::alloc_data_h(ExpEnum::Atom(Atom::String("".into(), None)));
                    let error =
                        Expression::alloc_data_h(ExpEnum::Atom(Atom::Symbol(":unexpected-eof")));
                    Ok(Expression::alloc_data(ExpEnum::Values(vec![input, error])))
                }
                ErrorKind::Interrupted => {
                    let input =
                        Expression::alloc_data_h(ExpEnum::Atom(Atom::String("".into(), None)));
                    let error =
                        Expression::alloc_data_h(ExpEnum::Atom(Atom::Symbol(":interrupted")));
                    Ok(Expression::alloc_data(ExpEnum::Values(vec![input, error])))
                }
                _ => {
                    eprintln!("Error on input: {}", err);
                    Err(LispError::new("Unexpected input error!"))
                }
            },
        };
    }
    Err(LispError::new(
        "prompt: requires a prompt string and option history file.",
    ))
}

fn builtin_prompt_history_push(
    environment: &mut Environment,
    args: &mut dyn Iterator<Item = Expression>,
) -> Result<Expression, LispError> {
    let liner_id = {
        let arg = param_eval(environment, args, "prompt-history-push")?;
        let arg_d = arg.get();
        if let ExpEnum::Atom(Atom::Symbol(s)) = arg_d.data {
            s
        } else {
            return Err(LispError::new(
                "prompt-history-push: context id must be a keyword.",
            ));
        }
    };
    let item = {
        let arg = param_eval(environment, args, "prompt-history-push")?;
        let arg_d = arg.get();
        if let ExpEnum::Atom(Atom::String(s, _)) = &arg_d.data {
            s.to_string()
        } else {
            return Err(LispError::new(
                "prompt-history-push: history item must be a string.",
            ));
        }
    };
    params_done(args, "prompt-history-push")?;
    let mut con = if environment.liners.contains_key(liner_id) {
        environment.liners.remove(liner_id).unwrap()
    } else {
        return Err(LispError::new("prompt-history-push: context id not found."));
    };
    let result = if let Err(err) = con.history.push(item) {
        eprintln!("Warning: failed to save history: {}", err);
        Ok(Expression::make_nil())
    } else {
        Ok(Expression::make_true())
    };
    environment.liners.insert(liner_id, con);
    result
}

fn builtin_prompt_history_push_throwaway(
    environment: &mut Environment,
    args: &mut dyn Iterator<Item = Expression>,
) -> Result<Expression, LispError> {
    let liner_id = {
        let arg = param_eval(environment, args, "prompt-history-push-throwaway")?;
        let arg_d = arg.get();
        if let ExpEnum::Atom(Atom::Symbol(s)) = arg_d.data {
            s
        } else {
            return Err(LispError::new(
                "prompt-history-push-throwaway: context id must be a keyword.",
            ));
        }
    };
    let item = {
        let arg = param_eval(environment, args, "prompt-history-push-throwaway")?;
        let arg_d = arg.get();
        if let ExpEnum::Atom(Atom::String(s, _)) = &arg_d.data {
            s.to_string()
        } else {
            return Err(LispError::new(
                "prompt-history-push-throwaway: history item must be a string.",
            ));
        }
    };
    params_done(args, "prompt-history-push-throwaway")?;
    let mut con = if environment.liners.contains_key(liner_id) {
        environment.liners.remove(liner_id).unwrap()
    } else {
        return Err(LispError::new(
            "prompt-history-push-throwaway: context id not found.",
        ));
    };
    let result = if let Err(err) = con.history.push_throwaway(item) {
        eprintln!("Warning: failed to save temp history: {}", err);
        Ok(Expression::make_nil())
    } else {
        Ok(Expression::make_true())
    };
    environment.liners.insert(liner_id, con);
    result
}

pub fn add_edit_builtins<S: BuildHasher>(
    interner: &mut Interner,
    data: &mut HashMap<&'static str, Reference, S>,
) {
    let root = interner.intern("root");
    data.insert(
        interner.intern("prompt"),
        Expression::make_function(
            builtin_prompt,
            "Usage: (prompt string) -> string

Starts an interactive prompt (like the repl prompt) with the supplied prompt and
returns the input string.

Section: shell

Example:
;(def 'input-string (prompt \"prompt> \"))
t
",
            root,
        ),
    );
    data.insert(
        interner.intern("prompt-history-push"),
        Expression::make_function(
            builtin_prompt_history_push,
            "Usage: (prompt-history-push :context_id string) -> nil/t

Pushes string onto the history for the prompt context :context_id.
Returns true on success or nil on failure.

Section: shell

Example:
;(prompt-history-push :repl \"Some command\")
t
",
            root,
        ),
    );
    data.insert(
        interner.intern("prompt-history-push-throwaway"),
        Expression::make_function(
            builtin_prompt_history_push_throwaway,
            "Usage: (prompt-history-push-throwaway :context_id string) -> nil/t

Pushes string onto the history for the prompt context :context_id.  A throwaway
item will will only persist until the next command is read (use it to allow
editing of failed commands without them going into history).
Returns true on success or nil on failure.

Section: shell

Example:
;(prompt-history-push-throwaway :repl \"Some broken command\")
t
",
            root,
        ),
    );
}