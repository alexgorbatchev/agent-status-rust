//! Cancellation for native, synchronous OpenCode queries.
use crate::{Result, check_cancel};
use agent_parser::OpencodeDb;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
pub(crate) fn query<T>(
    cancel: &CancellationToken,
    db: &OpencodeDb,
    query: impl FnOnce(&OpencodeDb) -> std::result::Result<T, agent_parser::Error>,
) -> Result<T> {
    check_cancel(cancel)?;
    let interrupt = db.interrupt_handle();
    let done = AtomicBool::new(false);
    let result = thread::scope(|scope| {
        let worker = scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                if cancel.is_cancelled() {
                    interrupt.interrupt();
                }
                thread::park_timeout(Duration::from_millis(10));
            }
        });
        struct Complete<'a> {
            done: &'a AtomicBool,
            worker: thread::Thread,
        }
        impl Drop for Complete<'_> {
            fn drop(&mut self) {
                self.done.store(true, Ordering::Release);
                self.worker.unpark();
            }
        }
        let _complete = Complete {
            done: &done,
            worker: worker.thread().clone(),
        };
        query(db)
    });
    check_cancel(cancel)?;
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use std::sync::mpsc;

    fn database() -> (tempfile::TempDir, OpencodeDb) {
        std::fs::create_dir_all(".tmp").unwrap();
        let dir = tempfile::tempdir_in(".tmp").unwrap();
        let path = dir.path().join("opencode.db");
        rusqlite::Connection::open(&path).unwrap();
        let db = OpencodeDb::open(path).unwrap();
        (dir, db)
    }

    #[test]
    fn query_returns_values_and_typed_errors_and_joins_after_unwind() {
        let (_dir, db) = database();
        let token = CancellationToken::new();
        assert_eq!(query(&token, &db, |_| Ok(42)).unwrap(), 42);
        assert!(matches!(
            query::<()>(&token, &db, |_| Err(agent_parser::Error::Invalid(
                "bad data".into()
            ))),
            Err(Error::Parser(agent_parser::Error::Invalid(_)))
        ));
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = query::<()>(&token, &db, |_| panic!("query failed unexpectedly"));
        }));
        assert!(panic.is_err());
        assert_eq!(query(&token, &db, |_| Ok(7)).unwrap(), 7);
        token.cancel();
        assert!(matches!(
            query::<()>(&token, &db, |_| panic!("cancelled query ran")),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn cancellation_interrupts_a_real_running_sqlite_statement() {
        let (_dir, db) = database();
        let token = CancellationToken::new();
        let (started, running) = mpsc::sync_channel(1);
        let mut started = Some(started);
        db.connection()
            .progress_handler(
                1000,
                Some(move || {
                    if let Some(started) = started.take() {
                        started.send(()).unwrap();
                    }
                    false
                }),
            )
            .unwrap();
        thread::scope(|scope| {
            let cancellation = token.clone();
            let canceller = scope.spawn(move || {
                running.recv_timeout(Duration::from_secs(5)).unwrap();
                cancellation.cancel();
            });
            let result = query(&token, &db, |db| {
                let result = db.connection().query_row("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000) SELECT sum(x) FROM n", [], |row| row.get::<_, i64>(0));
                assert!(
                    matches!(&result, Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::OperationInterrupted),
                    "{result:?}"
                );
                result.map_err(Into::into)
            });
            assert!(matches!(result, Err(Error::Cancelled)));
            canceller.join().unwrap();
        });
        assert_eq!(
            db.connection()
                .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
