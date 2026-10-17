//! `arcalo-mail-testserver`: the test server as a program, for the e2e tests and the README
//! screenshots. Prints one JSON line with its ports, then serves until it is stopped.
//!
//! ```text
//! arcalo-mail-testserver [--user maria@example.org] [--password secret] [--tls]
//!     [--without qresync,condstore,move,uidplus,idle,special_use] [--files-sent]
//!     [--imap-port N] [--smtp-port N] [--control-port N] [--pop3-port N] [--sieve-port N]
//!     [--no-jmap-push]
//! ```

use mail_testserver::{Options, start};

fn main() {
    let mut opts = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_default();
        match a.as_str() {
            "--user" => opts.user = value(),
            "--password" => opts.password = value(),
            "--tls" => opts.tls = true,
            "--files-sent" => opts.files_sent = true,
            "--no-jmap-push" => opts.jmap_push = false,
            "--imap-port" => opts.imap_port = value().parse().unwrap_or(0),
            "--smtp-port" => opts.smtp_port = value().parse().unwrap_or(0),
            "--control-port" => opts.control_port = value().parse().unwrap_or(0),
            "--pop3-port" => opts.pop3_port = value().parse().unwrap_or(0),
            "--sieve-port" => opts.sieve_port = value().parse().unwrap_or(0),
            "--without" => {
                for cap in value().split(',') {
                    match cap.trim() {
                        "qresync" => opts.caps.qresync = false,
                        "condstore" => (opts.caps.condstore, opts.caps.qresync) = (false, false),
                        "move" => opts.caps.moves = false,
                        "uidplus" => opts.caps.uidplus = false,
                        "idle" => opts.caps.idle = false,
                        "special_use" => opts.caps.special_use = false,
                        other => eprintln!("unknown capability {other}"),
                    }
                }
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let s = start(opts).await.expect("start");
        println!(
            "{}",
            serde_json::json!({
                "imap": s.imap,
                "smtp": s.smtp,
                "imaps": s.imaps,
                "smtps": s.smtps,
                "control": s.control,
                "pop3": s.pop3,
                "sieve": s.sieve,
                "fingerprint": s.fingerprint,
            })
        );
        let _ = tokio::signal::ctrl_c().await;
    });
}
