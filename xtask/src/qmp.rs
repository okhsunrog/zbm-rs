//! Bounded QMP requests: greeting, capabilities, request IDs and async events.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

pub struct Qmp {
    reader: BufReader<UnixStream>,
    id: u64,
}

impl Qmp {
    pub fn connect(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .with_context(|| format!("QMP unavailable: {}", path.display()))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut client = Self {
            reader: BufReader::new(stream),
            id: 0,
        };
        let greeting = client.read()?;
        if greeting.get("QMP").is_none() {
            bail!("Invalid QMP greeting: {greeting}");
        }
        client.request("qmp_capabilities", json!({}))?;
        Ok(client)
    }

    fn read(&mut self) -> Result<Value> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            bail!("QMP disconnected");
        }
        Ok(serde_json::from_str(&line)?)
    }

    pub fn request(&mut self, command: &str, args: Value) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(5);
        self.id += 1;
        writeln!(
            self.reader.get_mut(),
            "{}",
            json!({"execute": command, "arguments": args, "id": self.id})
        )?;
        // Bound event traffic as well as individual socket reads.
        for _ in 0..100 {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("QMP request timed out")?;
            self.reader.get_ref().set_read_timeout(Some(remaining))?;
            let response = self.read()?;
            if response.get("id").and_then(Value::as_u64) != Some(self.id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                bail!("QMP {command}: {error}");
            }
            return response
                .get("return")
                .cloned()
                .context("Missing QMP return");
        }
        bail!("Too many asynchronous QMP messages")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn events_do_not_consume_responses_and_errors_propagate() {
        let path = std::env::temp_dir().join(format!("zbm-qmp-test-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            writeln!(stream, "{}", json!({"QMP": {}})).unwrap();
            let mut reader = BufReader::new(stream);
            for id in 1..=2 {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["id"], id);
                writeln!(reader.get_mut(), "{}", json!({"event": "RESUME"})).unwrap();
                let response = if id == 1 {
                    json!({"return": {}, "id": id})
                } else {
                    json!({"error": {"desc": "fixture error"}, "id": id})
                };
                writeln!(reader.get_mut(), "{response}").unwrap();
            }
        });
        let mut qmp = Qmp::connect(&path).unwrap();
        assert!(
            qmp.request("bad-command", json!({}))
                .unwrap_err()
                .to_string()
                .contains("fixture error")
        );
        worker.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
