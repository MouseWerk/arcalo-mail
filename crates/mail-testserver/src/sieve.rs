//! The ManageSieve side (RFC 5804): scripts kept in memory (`GET /sieve` on the control port
//! shows them), the account's user name and password.

use std::sync::Arc;

use mail_core::sieve::fake::FakeSieve;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::{Io, Shared};

pub(crate) async fn session(io: Io, shared: Arc<Shared>) -> std::io::Result<()> {
    let mut io = BufReader::new(io);
    let greeting = {
        let mut s = shared.sieve.lock().unwrap_or_else(|e| e.into_inner());
        s.password = shared.password.lock().unwrap_or_else(|e| e.into_inner()).clone();
        s.authed = false;
        s.greeting()
    };
    io.get_mut().write_all(greeting.as_bytes()).await?;
    let mut authed = false;
    loop {
        let mut line = String::new();
        if io.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let mut literal = None;
        if let Some(n) = FakeSieve::literal(&line) {
            let mut buf = vec![0u8; n];
            io.read_exact(&mut buf).await?;
            let mut end = String::new();
            io.read_line(&mut end).await?;
            literal = Some(String::from_utf8_lossy(&buf).into_owned());
        }
        let (answer, bye) = {
            let mut s = shared.sieve.lock().unwrap_or_else(|e| e.into_inner());
            // Each connection signs in for itself.
            s.authed = authed;
            let r = s.handle(line.trim_end(), literal.as_deref());
            authed = s.authed;
            r
        };
        io.get_mut().write_all(answer.as_bytes()).await?;
        io.get_mut().flush().await?;
        if bye {
            return Ok(());
        }
    }
}
