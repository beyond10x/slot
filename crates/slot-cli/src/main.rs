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
        Cmd::Run(args) => Ok(build(&args.command, cli.local)),
        Cmd::Status(args) => slot::status::fetch(&args.http).map(|s| {
            print!("{}", slot::status::render(&s));
            0
        }),
        Cmd::Headroom(args) => slot::status::headroom(&args.http, args.percent).map(|v| {
            print!("{}", slot::status::render_headroom(&v));
            0
        }),
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
            Ok(build(&argv, cli.local))
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

fn build(argv: &[String], local: bool) -> i32 {
    if local {
        return client::run_local(argv);
    }
    client::run(argv, &Settings::from_env())
}

fn serve(args: &slot::cli::ServeArgs) -> Result<()> {
    let cfg = args.config()?;
    let rt = tokio_runtime()?;
    rt.block_on(async move { slot::server::Server::new(cfg)?.run().await })
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
