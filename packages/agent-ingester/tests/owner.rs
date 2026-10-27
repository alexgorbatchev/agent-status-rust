use agent_ingester::{
    classifier::Config,
    ingestion::Ingestion,
    owner::{Error, Owner},
};
use std::time::Duration;

#[tokio::test]
async fn sqlite_has_one_ordered_owner_and_never_blocks_async_executor() {
    let owner = Owner::start(Ingestion::open(":memory:", Config::default()).unwrap()).unwrap();
    let first = owner.clone();
    let task = tokio::spawn(async move {
        first
            .call(|ingestion| {
                ingestion
                    .connection()
                    .execute_batch("CREATE TABLE ordered(value INTEGER)")?;
                std::thread::sleep(Duration::from_millis(100));
                ingestion
                    .connection()
                    .execute("INSERT INTO ordered VALUES(1)", [])?;
                Ok(())
            })
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!task.is_finished());
    assert_eq!(
        owner
            .call(|ingestion| Ok(ingestion.connection().query_row(
                "SELECT value FROM ordered",
                [],
                |row| row.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
        1
    );
    task.await.unwrap();
    assert!(matches!(
        owner
            .call::<()>(|db| {
                db.connection()
                    .execute("INSERT INTO missing VALUES(1)", [])?;
                Ok(())
            })
            .await,
        Err(Error::Ingestion(_))
    ));
    owner.shutdown().await.unwrap();
    owner.shutdown().await.unwrap();
    assert!(matches!(owner.call(|_| Ok(())).await, Err(Error::Closed)));
}
