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
        Cmd::Gate(args) => gate(&args),
        Cmd::Hold(args) => Ok(slot::gate::hold(&args.fifo)),
        Cmd::Siblings(args) => Ok(siblings(&args.ws_root, &args.id, &args.key)),
        Cmd::CargoGit(args) => Ok(slot::git_deps::parse_words(&args.wanted).map_or_else(
            || {
                eprintln!("slot: __cargo-git takes `<entry>:<rev>` words");
                2
            },
            |w| cargo_git(&w),
        )),
        Cmd::Exec(args) => Ok(args
            .nofile
            .exec(&args.arg0, &args.command[0], &args.command[1..])),
        Cmd::Run(args) if args.warm => {
            let mut s = Settings::from_env();
            s.warm = true;
            s.priority = Some(slot::types::Priority::Low);
            s.timeout = args.timeout.or(cli.timeout);
            Ok(client::run(&args.command, &s))
        }
        Cmd::Warm(args) => warm(args.dir.as_deref(), cli.timeout),
        Cmd::Run(args) => Ok(build(
            &args.command,
            cli.local,
            &cli.fetch,
            args.tokens.or(cli.tokens),
            args.priority.or(cli.priority),
            args.timeout.or(cli.timeout),
            cli.here,
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
        Cmd::RunLimit(args) => slot::status::run_limit(&args.http, args.limit, args.max).map(|v| {
            print!("{}", slot::status::render_run_limit(&v));
            0
        }),
        Cmd::JobParallel(args) => slot::status::job_parallel(&args.http, args.limit).map(|v| {
            print!("{}", slot::status::render_job_parallel(&v));
            0
        }),
        Cmd::EvictBelow(args) => {
            slot::status::evict_below(&args.http, args.mark, args.idle).map(|v| {
                print!("{}", v.render());
                0
            })
        }
        Cmd::CpuLimit(args) => args
            .request()
            .map_err(anyhow::Error::msg)
            .and_then(|change| slot::status::cpu_limit(&args.http, change.as_ref()))
            .map(|v| {
                print!("{}", slot::status::render_cpu_limit(&v));
                0
            }),
        Cmd::Admission(args) => {
            slot::status::admission(&args.http, args.request().as_ref()).map(|v| {
                print!("{}", slot::status::render_admission(&v));
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
        Cmd::Usage(args) => usage(&args),
        Cmd::Cache(slot::cli::CacheCmd::Refused(args)) => cache_refused(&args),
        Cmd::Nodes(cmd) => nodes(&cmd),
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
                cli.timeout,
                cli.here,
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
    timeout: Option<u64>,
    here: bool,
) -> i32 {
    if local {
        return client::run_local(argv);
    }
    let mut s = Settings::from_env();
    s.here = here;
    s.fetch = fetch.to_vec();
    s.tokens = tokens;
    s.priority = priority;
    s.timeout = timeout;
    client::run(argv, &s)
}

/// `slot warm [DIR]`: the warm-up of the checkout at `dir`, else of the
/// working directory's.
fn warm(dir: Option<&std::path::Path>, timeout: Option<u64>) -> Result<i32> {
    if let Some(d) = dir {
        std::env::set_current_dir(d).with_context(|| d.display().to_string())?;
    }
    let mut s = Settings::from_env();
    s.timeout = timeout;
    Ok(client::warm(&s))
}

/// `slot gate`: serves the gate directories until killed.
fn gate(args: &slot::cli::GateArgs) -> Result<i32> {
    let exe = std::env::current_exe().context("the slot binary's path")?;
    slot::gate::serve(
        &args.dir,
        &args.socket,
        &args.state_dir.join("jobserver.fifo"),
        &exe,
    )?;
    Ok(0)
}

/// `slot priority set|unset|list`: changes the rules, then prints them.
/// `slot priority promote-after`: sets the delay, then prints it.
fn priority(cmd: &slot::cli::PriorityCmd) -> Result<i32> {
    use slot::cli::PriorityCmd;
    let v = match cmd {
        PriorityCmd::PromoteAfter(a) => {
            let v = slot::status::promote_after(&a.http, a.seconds)?;
            print!("{}", slot::status::render_promote_after(&v, a.seconds));
            return Ok(0);
        }
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
    // The account's own database, never the session's `CARGO_HOME`.
    let git_db = slot::git_deps::node_db(&home);
    let roots = slot::shell::Roots {
        ws: &ws,
        cache: &cache,
        same_path: &args.same_path_root,
        cargo_git_db: &git_db,
    };
    match slot::shell::plan(&line, &home, &roots) {
        Ok(Plan::Rsync(rsync_args)) => {
            // A relative path is the account home's, as the shell checked it
            // and as sshd starts a session there.
            let e = std::process::Command::new("rsync")
                .args(rsync_args)
                .current_dir(&home)
                .exec();
            eprintln!("slot shell: rsync: {e}");
            127
        }
        Ok(Plan::Proxy) => match UnixStream::connect(&args.socket) {
            Err(e) if slot::shell::daemon_away(&e) => {
                eprintln!(
                    "{} connect {}: {e}",
                    slot::shell::DAEMON_AWAY,
                    args.socket.display()
                );
                slot::shell::DAEMON_AWAY_EXIT
            }
            Err(e) => {
                eprintln!("slot shell: connect {}: {e}", args.socket.display());
                3
            }
            Ok(c) => bridge(&c).unwrap_or_else(|e| {
                eprintln!("slot shell: {e:#}");
                3
            }),
        },
        Ok(Plan::Cancel(id)) => {
            let mut s = Settings::from_env();
            s.host = None;
            s.socket.clone_from(&args.socket);
            client::cancel(id, &s)
        }
        Ok(Plan::State) => match slot::server::nodes::state_step(&args.http) {
            Ok(s) => {
                println!("{s}");
                0
            }
            Err(e) => {
                eprintln!("slot shell: state: {e:#}");
                1
            }
        },
        Ok(Plan::Output { id, from }) => {
            match slot::server::nodes::output_step(&args.http, id, from) {
                Ok(s) => {
                    println!("{s}");
                    0
                }
                Err(e) => {
                    eprintln!("slot shell: output: {e:#}");
                    1
                }
            }
        }
        Ok(Plan::Siblings { id, key, path }) => {
            if let Some(p) = path
                && let Err(e) = slot::mirror::link(&ws, &id, &p)
            {
                eprintln!("slot: {e}");
                return 1;
            }
            siblings(&ws, &id, &key)
        }
        Ok(Plan::CargoGit(wanted)) => cargo_git(&wanted),
        Ok(Plan::Forget(path)) => {
            match slot::shell::forget_step(&args.http, &path, &home.join(&ws)) {
                Ok(lines) => {
                    for l in lines {
                        println!("{l}");
                    }
                    0
                }
                Err(e) => {
                    eprintln!("slot shell: forget: {e}");
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("slot shell: refused: {e}");
            126
        }
    }
}

/// `slot __cargo-git`: prints the entries of `wanted` that the account's
/// cargo git database (`~/.cargo/git/db`) lacks, by name.
fn cargo_git(wanted: &[slot::git_deps::Wanted]) -> i32 {
    let home = std::env::var_os("HOME").map_or_else(|| std::path::PathBuf::from("/"), Into::into);
    let db = slot::git_deps::node_db(&home);
    print!("{}", slot::git_deps::node_answer(&db, wanted));
    0
}

/// `slot __siblings`: records mirror `id` of repository `key` and prints
/// its sibling mirrors, one id per line.
fn siblings(ws_root: &std::path::Path, id: &str, key: &str) -> i32 {
    match slot::mirror::siblings(ws_root, id, key) {
        Ok(ids) => {
            for i in ids {
                println!("{i}");
            }
            0
        }
        Err(e) => {
            eprintln!("slot: {e}");
            1
        }
    }
}

fn serve(args: &slot::cli::ServeArgs) -> Result<()> {
    // Before anything opens a descriptor: a handover's are adopted by number.
    let inherited = slot::server::handover::take_inherited();
    slot::server::handover::catch_hup()?;
    let cfg = args.config()?;
    if let Some(s) = &cfg.sandbox {
        eprintln!(
            "slot: every job runs in a sandbox ({})",
            s.bwrap().display()
        );
    }
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
    bridge(&c)
}

/// Copies stdin to the daemon's connection `c` and its answer to stdout.
fn bridge(c: &UnixStream) -> Result<i32> {
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

/// `slot nodes …`. `route` exits 3 when no node matches.
fn nodes(cmd: &slot::cli::NodesCmd) -> Result<i32> {
    use slot::cli::NodesCmd;
    use slot::hostconfig;
    let file = |f: &std::path::Path| -> Result<std::path::PathBuf> {
        if f.as_os_str() == "off" {
            anyhow::bail!("no host configuration file (--cluster-file off)");
        }
        Ok(f.to_path_buf())
    };
    let read = |path: &std::path::Path| -> Result<String> {
        match std::fs::read_to_string(path) {
            Ok(t) => Ok(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(anyhow::anyhow!("{}: {e}", path.display())),
        }
    };
    match cmd {
        NodesCmd::Route(a) => {
            let path = file(&a.cluster_file)?;
            let dir = match &a.dir {
                Some(d) => d.clone(),
                None => std::env::current_dir()?,
            };
            let routed = read(&path)
                .map_err(|e| format!("{e:#}"))
                .and_then(|text| hostconfig::route_in(&text, &dir));
            match routed {
                Ok(lines) => {
                    print!("{lines}");
                    Ok(0)
                }
                Err(e) => {
                    eprintln!("slot: {}: {e}", path.display());
                    Ok(3)
                }
            }
        }
        NodesCmd::List(a) => {
            let path = file(&a.cluster_file)?;
            let reg = hostconfig::parse_registry(&read(&path)?)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            let rows = slot::status::fetch_nodes(&a.http).unwrap_or_default();
            print!("{}", hostconfig::render_list(&reg, &rows));
            Ok(0)
        }
        NodesCmd::Add(a) => {
            let path = file(&a.cluster_file)?;
            let new = hostconfig::NewNode {
                name: a.name.clone(),
                ssh: a.ssh.clone(),
                slot_path: a.slot_path.clone(),
                deadline: a.deadline.clone(),
                price_per_hour_cents: a.price_per_hour_cents,
                spill: a.spill,
                repositories: a.repository.clone(),
                default: a.default,
            };
            let text = hostconfig::add(&read(&path)?, &new)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            hostconfig::write(&path, &text).with_context(|| path.display().to_string())?;
            eprintln!("slot: node {} added to {}", a.name, path.display());
            Ok(0)
        }
        NodesCmd::Remove(a) => {
            let path = file(&a.cluster_file)?;
            let text = hostconfig::remove(&read(&path)?, &a.name)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            hostconfig::write(&path, &text).with_context(|| path.display().to_string())?;
            eprintln!("slot: node {} removed from {}", a.name, path.display());
            Ok(0)
        }
        NodesCmd::Forget(a) => {
            let path = file(&a.cluster_file)?;
            let reg = hostconfig::parse_registry(&read(&path)?)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            let node = reg
                .nodes
                .iter()
                .find(|n| n.name == a.name)
                .ok_or_else(|| anyhow::anyhow!("{}: no node {}", path.display(), a.name))?;
            let argv = node.forget_command(&a.path).map_err(anyhow::Error::msg)?;
            let (prog, rest) = argv
                .split_first()
                .ok_or_else(|| anyhow::anyhow!("node {}: empty ssh line", a.name))?;
            let st = std::process::Command::new(prog)
                .args(rest)
                .stdin(std::process::Stdio::null())
                .status()
                .with_context(|| prog.clone())?;
            Ok(st.code().unwrap_or(1))
        }
    }
}

fn cache_refused(args: &slot::cli::CacheRefusedArgs) -> Result<i32> {
    let since = slot::usage::parse_since(&args.since).map_err(anyhow::Error::msg)?;
    let by: slot::usage::By = args.by.parse().map_err(anyhow::Error::msg)?;
    if matches!(by, slot::usage::By::Session | slot::usage::By::Node) {
        anyhow::bail!("--by {}: repo or project", args.by);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let from = now - since;
    let (records, jobs) = slot::refused::read(&args.state_dir, from, now);
    let r = slot::refused::attribute(&records, &jobs, from, now, by);
    if args.json {
        println!("{}", slot::refused::json(&r));
    } else {
        print!("{}", slot::refused::render(&r));
    }
    Ok(0)
}
