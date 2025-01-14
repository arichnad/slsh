use std::io;

use nix::{
    sys::signal::{self, SigHandler, Signal},
    unistd,
};

use ::slsh::*;

fn main() -> io::Result<()> {
    let config = get_config();
    if let Ok(config) = config {
        if config.command.is_none() && config.script.is_none() {
            /* See if we are running interactively.  */
            let shell_terminal = nix::libc::STDIN_FILENO;
            if let Ok(true) = unistd::isatty(shell_terminal) {
                /* Loop until we are in the foreground.  */
                let mut shell_pgid = unistd::getpgrp();
                while unistd::tcgetpgrp(shell_terminal) != Ok(shell_pgid) {
                    //kill (- shell_pgid, SIGTTIN);
                    if let Err(err) = signal::kill(shell_pgid, Signal::SIGTTIN) {
                        eprintln!("Error sending sigttin: {}.", err);
                    }
                    shell_pgid = unistd::getpgrp();
                }

                /* Ignore interactive and job-control signals.  */
                unsafe {
                    signal::signal(Signal::SIGINT, SigHandler::SigIgn).unwrap();
                    signal::signal(Signal::SIGQUIT, SigHandler::SigIgn).unwrap();
                    signal::signal(Signal::SIGTSTP, SigHandler::SigIgn).unwrap();
                    signal::signal(Signal::SIGTTIN, SigHandler::SigIgn).unwrap();
                    signal::signal(Signal::SIGTTOU, SigHandler::SigIgn).unwrap();
                    // Ignoring sigchild will mess up waitpid and cause Command::spawn to panic under some conditions.
                    //signal::signal(Signal::SIGCHLD, SigHandler::SigIgn).unwrap();
                }

                /* Put ourselves in our own process group.  */
                let pgid = unistd::getpid();
                if let Err(err) = unistd::setpgid(pgid, pgid) {
                    match err {
                        nix::Error::Sys(nix::errno::Errno::EPERM) => { /* ignore */ }
                        _ => {
                            eprintln!("Couldn't put the shell in its own process group: {}\n", err)
                        }
                    }
                }
                /* Grab control of the terminal.  */
                if let Err(err) = unistd::tcsetpgrp(shell_terminal, pgid) {
                    let msg = format!("Couldn't grab control of terminal: {}\n", err);
                    eprintln!("{}", msg);
                    return Err(io::Error::new(io::ErrorKind::Other, msg));
                }
                start_interactive();
            } else {
                // No tty, just read stdin and do something with it..
                read_stdin();
            }
        } else if config.command.is_some() {
            let command = config.command.unwrap();
            if let Err(err) = run_one_command(&command, &config.args) {
                eprintln!("Error running {}: {}", command, err);
                return Err(err);
            }
        } else if config.script.is_some() {
            let script = config.script.unwrap();
            if let Err(err) = run_one_script(&script, &config.args) {
                eprintln!("Error running {}: {}", script, err);
                return Err(err);
            }
        }
    }
    Ok(())
}
