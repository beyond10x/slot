//! The `slot` binary.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use slot::cli::{Cli, Cmd};
use slot::client::{self, Settings};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Serve(args) => serve(&args).map(|()| 0),
        Cmd::Proxy(args) => proxy(&args.socket),
        Cmd::Shell(args) => Ok(shell(&args)),
        Cmd::Run(args) => Ok(build(
            &args.command,
            cli.local,
            &cli.fetch,
            args.tokens.or(cli.tokens),
            args.priority.or(cli.priority),
        )),
        Cmd::Status(args) => slot::status::fetch(&args.http).map(|s| {
            print!("{}", slot::status::render(&s));
            0
        }),
        Cmd::Headroom(args) => {
            slot::status::headroom(&args.http, args.percent, args.call, args.base).map(|v| {
                print!("{}", slot::status::render_headroom(&v));
                0
            })
        }
        Cmd::Priority(cmd) => priority(&cmd),
        Cmd::Cancel(args) => Ok(client::cancel(args.id, &Settings::from_env())),
        Cmd::Sessions(args) => {
            let r = slot::sessions::report(&args.http, args.window, args.min_cores);
            if args.json {
                println!("{}", slot::sessions::json(&r));
            } else {
                print!("{}", slot::sessions::render(&r));
            }
            Ok(0)
        }
        Cmd::Usage(args) => usage(&args),
        Cmd::Hook(slot::cli::HookCmd::ClaudePreToolUse) => return slot::hook::run(),
        Cmd::External(argv) => {
            let argv: Vec<String> = argv
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            Ok(build(
                &argv,
                cli.local,
                &cli.fetch,
                cli.tokens,
                cli.priority,
            ))
        }
    };
    match result {
        Ok(code) => ExitCode::from(u8::try_from(code.clamp(0, 255)).unwrap_or(1)),
        Err(e) => {
            eprintln!("slot: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn build(
    argv: &[String],
    local: bool,
    fetch: &[String],
    tokens: Option<u32>,
    priority: Option<slot::types::Priority>,
) -> i32 {
    if local {
        return client::run_local(argv);
    }
    let mut s = Settings::from_env();
    s.fetch = fetch.to_vec();
    s.tokens = tokens;
    s.priority = priority;
    client::run(argv, &s)
}

/// `slot priority set|unset|list`: changes the rules, then prints them.
fn priority(cmd: &slot::cli::PriorityCmd) -> Result<i32> {
    use slot::cli::PriorityCmd;
    let v = match cmd {
        PriorityCmd::Set(a) => slot::status::priority(&a.http, Some((&a.pattern, Some(a.level))))?,
        PriorityCmd::Unset(a) => slot::status::priority(&a.http, Some((&a.pattern, None)))?,
        PriorityCmd::List(a) => slot::status::priority(&a.http, None)?,
    };
    print!("{}", slot::status::render_priority(&v));
    Ok(0)
}

/// `slot shell`: the forced command of a build server's ssh key. Exits
/// 126 for a command that is not a remote step.
fn shell(args: &slot::cli::ShellArgs) -> i32 {
    use slot::shell::Plan;
    use std::os::unix::process::CommandExt;
    let line = std::env::var("SSH_ORIGINAL_COMMAND").unwrap_or_default();
    let home = std::env::var_os("HOME").map_or_else(|| std::path::PathBuf::from("/"), Into::into);
    let ws = args
        .ws_root
        .clone()
        .unwrap_or_else(|| args.state_dir.join("ws"));
    let cache = args
        .cache_root
        .clone()
        .unwrap_or_else(|| args.state_dir.join("cache"));
    match slot::shell::plan(&line, &home, &ws, &cache) {
        Ok(Plan::Rsync(rsync_args)) => {
            let e = std::process::Command::new("rsync").args(rsync_args).exec();
            eprintln!("slot shell: rsync: {e}");
            127
        }
        Ok(Plan::Proxy) => proxy(&args.socket).unwrap_or_else(|e| {
            eprintln!("slot shell: {e:#}");
            3
        }),
        Ok(Plan::Cancel(id)) => {
            let mut s = Settings::from_env();
            s.host = None;
            s.socket.clone_from(&args.socket);
            client::cancel(id, &s)
        }
        Err(e) => {
            eprintln!("slot shell: refused: {e}");
            126
        }
    }
}

fn serve(args: &slot::cli::ServeArgs) -> Result<()> {
    // Before anything opens a descriptor: a handover's are adopted by number.
    let inherited = slot::server::handover::take_inherited();
    slot::server::handover::catch_hup()?;
    let cfg = args.config()?;
    let rt = tokio_runtime()?;
    rt.block_on(async move {
        slot::server::Server::with_inherited(cfg, inherited)?
            .run()
            .await
    })
}

fn tokio_runtime() -> Result<slot::server::Runtime> {
    slot::server::runtime()
}

fn proxy(socket: &std::path::Path) -> Result<i32> {
    let c = UnixStream::connect(socket).with_context(|| format!("connect {}", socket.display()))?;
    let mut up = c.try_clone()?;
    let down = c.try_clone()?;
    let t = std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let _ = std::io::copy(&mut stdin, &mut up);
        let _ = up.shutdown(std::net::Shutdown::Write);
    });
    let mut stdout = std::io::stdout();
    let mut down = down;
    let mut buf = vec![0u8; 32 << 10];
    loop {
        match down.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stdout
                    .write_all(&buf[..n])
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    drop(t);
    Ok(0)
}

fn usage(args: &slot::cli::UsageArgs) -> Result<i32> {
    let since = slot::usage::parse_since(&args.since).map_err(anyhow::Error::msg)?;
    let by: slot::usage::By = args.by.parse().map_err(anyhow::Error::msg)?;
    let u = slot::status::fetch_usage(&args.http, since, by)?;
    if args.json {
        println!("{}", slot::usage::json(&u));
    } else {
        print!("{}", slot::usage::render(&u, args.top));
    }
    Ok(0)
}
