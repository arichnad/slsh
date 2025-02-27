use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::io;
use std::process::Child;
use std::rc::Rc;

use crate::builtins::add_builtins;
use crate::builtins_file::add_file_builtins;
use crate::builtins_list::add_list_builtins;
use crate::builtins_math::add_math_builtins;
use crate::builtins_str::add_str_builtins;
use crate::process::*;
use crate::types::*;

#[derive(Clone, Debug)]
pub enum IOState {
    FileAppend(String),
    FileOverwrite(String),
    Pipe,
    Inherit,
    Null,
}

#[derive(Clone, Debug)]
pub struct EnvState {
    pub recur_num_args: Option<usize>,
    pub gensym_count: u32,
    pub stdout_status: Option<IOState>,
    pub stderr_status: Option<IOState>,
    pub eval_level: u32,
    pub is_spawn: bool,
    pub pipe_pgid: Option<u32>,
}

impl Default for EnvState {
    fn default() -> Self {
        EnvState {
            recur_num_args: None,
            gensym_count: 0,
            stdout_status: None,
            stderr_status: None,
            eval_level: 0,
            is_spawn: false,
            pipe_pgid: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormType {
    Any,
    FormOnly,
    ExternalOnly,
}

#[derive(Clone, Debug)]
pub struct Scope {
    pub data: HashMap<String, Rc<Expression>>,
    pub outer: Option<Rc<RefCell<Scope>>>,
}

impl Default for Scope {
    fn default() -> Self {
        let mut data = HashMap::new();
        add_builtins(&mut data);
        add_math_builtins(&mut data);
        add_str_builtins(&mut data);
        add_list_builtins(&mut data);
        add_file_builtins(&mut data);
        Scope { data, outer: None }
    }
}

impl Scope {
    pub fn with_data<S: ::std::hash::BuildHasher>(
        environment: Option<&Environment>,
        mut data_in: HashMap<String, Rc<Expression>, S>,
    ) -> Scope {
        let mut data: HashMap<String, Rc<Expression>> = HashMap::with_capacity(data_in.len());
        for (k, v) in data_in.drain() {
            data.insert(k, v);
        }
        let outer = if let Some(environment) = environment {
            if let Some(scope) = environment.current_scope.last() {
                Some(scope.clone())
            } else {
                None
            }
        } else {
            None
        };
        Scope { data, outer }
    }
}

#[derive(Clone, Debug)]
pub struct Environment {
    pub state: EnvState,
    pub stopped_procs: Rc<RefCell<Vec<u32>>>,
    pub in_pipe: bool,
    pub run_background: bool,
    pub is_tty: bool,
    pub loose_symbols: bool,
    pub procs: Rc<RefCell<HashMap<u32, Child>>>,
    pub data_in: Option<Expression>,
    pub form_type: FormType,
    pub save_exit_status: bool,
    // This is the environment's root (global scope), it will also be part of
    // higher level scopes and in the curren_scope vector (the first item).
    // It's special so keep a reference here as well for handy access.
    pub root_scope: Rc<RefCell<Scope>>,
    // Use as a stack of scopes, entering a new pushes and it get popped on exit
    // The actual lookups are done use the scope and it's outer chain NOT this stack.
    pub current_scope: Vec<Rc<RefCell<Scope>>>,
}

pub fn build_default_environment() -> Environment {
    let procs: Rc<RefCell<HashMap<u32, Child>>> = Rc::new(RefCell::new(HashMap::new()));
    let root_scope = Rc::new(RefCell::new(Scope::default()));
    let mut current_scope = Vec::new();
    current_scope.push(root_scope.clone());
    Environment {
        state: EnvState::default(),
        stopped_procs: Rc::new(RefCell::new(Vec::new())),
        in_pipe: false,
        run_background: false,
        is_tty: true,
        loose_symbols: false,
        procs,
        data_in: None,
        form_type: FormType::Any,
        save_exit_status: true,
        root_scope,
        current_scope,
    }
}

pub fn build_new_spawn_scope<S: ::std::hash::BuildHasher>(
    mut data_in: HashMap<String, Expression, S>,
) -> Environment {
    let procs: Rc<RefCell<HashMap<u32, Child>>> = Rc::new(RefCell::new(HashMap::new()));
    let mut state = EnvState::default();
    let mut data: HashMap<String, Rc<Expression>> = HashMap::with_capacity(data_in.len());
    for (k, v) in data_in.drain() {
        data.insert(k, Rc::new(v));
    }
    state.is_spawn = true;
    let root_scope = Rc::new(RefCell::new(Scope::with_data(None, data)));
    let mut current_scope = Vec::new();
    current_scope.push(root_scope.clone());
    Environment {
        state,
        stopped_procs: Rc::new(RefCell::new(Vec::new())),
        in_pipe: false,
        run_background: false,
        is_tty: false,
        loose_symbols: false,
        procs,
        data_in: None,
        form_type: FormType::Any,
        save_exit_status: true,
        root_scope,
        current_scope,
    }
}

pub fn build_new_scope(outer: Option<Rc<RefCell<Scope>>>) -> Rc<RefCell<Scope>> {
    let data: HashMap<String, Rc<Expression>> = HashMap::new();
    Rc::new(RefCell::new(Scope { data, outer }))
}

pub fn clone_symbols<S: ::std::hash::BuildHasher>(
    scope: &Scope,
    data_in: &mut HashMap<String, Expression, S>,
) {
    for (k, v) in &scope.data {
        let v = &**v;
        data_in.insert(k.clone(), v.clone());
    }
    if let Some(outer) = &scope.outer {
        clone_symbols(&outer.borrow(), data_in);
    }
}

pub fn get_expression(environment: &Environment, key: &str) -> Option<Rc<Expression>> {
    let mut loop_scope = Some(environment.current_scope.last().unwrap().clone());
    while loop_scope.is_some() {
        let scope = loop_scope.unwrap();
        if let Some(exp) = scope.borrow().data.get(key) {
            return Some(exp.clone());
        }
        loop_scope = scope.borrow().outer.clone();
    }
    None
}

pub fn set_expression_global(
    environment: &mut Environment,
    key: String,
    expression: Rc<Expression>,
) {
    environment
        .root_scope
        .borrow_mut()
        .data
        .insert(key, expression);
}

pub fn is_expression(environment: &Environment, key: &str) -> bool {
    if key.starts_with('$') {
        env::var(&key[1..]).is_ok()
    } else {
        let mut loop_scope = Some(environment.current_scope.last().unwrap().clone());
        while loop_scope.is_some() {
            let scope = loop_scope.unwrap();
            if let Some(_exp) = scope.borrow().data.get(key) {
                return true;
            }
            loop_scope = scope.borrow().outer.clone();
        }
        false
    }
}

pub fn get_symbols_scope(environment: &Environment, key: &str) -> Option<Rc<RefCell<Scope>>> {
    let mut loop_scope = Some(environment.current_scope.last().unwrap().clone());
    while loop_scope.is_some() {
        let scope = loop_scope.unwrap();
        if let Some(_exp) = scope.borrow().data.get(key) {
            return Some(scope.clone());
        }
        loop_scope = scope.borrow().outer.clone();
    }
    None
}

pub fn add_process(environment: &Environment, process: Child) -> u32 {
    let pid = process.id();
    environment.procs.borrow_mut().insert(pid, process);
    pid
}

pub fn reap_procs(environment: &Environment) -> io::Result<()> {
    let mut procs = environment.procs.borrow_mut();
    let keys: Vec<u32> = procs.keys().copied().collect();
    let mut pids: Vec<u32> = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(child) = procs.get_mut(&key) {
            pids.push(child.id());
        }
    }
    drop(procs);
    for pid in pids {
        try_wait_pid(environment, pid);
    }
    // XXX remove them or better replace pid with exit status
    Ok(())
}
