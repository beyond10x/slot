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
        Cmd::Run(args) => Ok(build(&args.command, cli.local, &cli.fetch)),
        Cmd::Status(args) => slot::status::fetch(&args.http).map(|s| {
            print!("{}", slot::status::render(&s));
            0
        }),
        Cmd::Headroom(args) => {
            slot::status::headroom(&args.http, args.percent, args.call).map(|v| {
                print!("{}", slot::status::render_headroom(&v));
                0
            })
        }
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
        Cmd::Hook(slot::cli::HookCmd::ClaudePreToolUse) => return slot::hook::run(),
        Cmd::External(argv) => {
            let argv: Vec<String> = argv
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            Ok(build(&argv, cli.local, &cli.fetch))
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

fn build(argv: &[String], local: bool, fetch: &[String]) -> i32 {
    if local {
        return client::run_local(argv);
    }
    let mut s = Settings::from_env();
    s.fetch = fetch.to_vec();
    client::run(argv, &s)
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
