//! Verify test infrastructure using real files, SQLite transactions and subprocesses.
//! The miniature candidate/head example is not Transflow's publication algorithm.

mod support;

use sqlx::Connection;
use std::{
    error::Error,
    fs,
    io::{self, Read, Write},
    path::Path,
    thread,
    time::Duration,
};
use support::{
    database,
    determinism::{Clock, IdSource, SeededRandom, SequentialIds, VirtualClock},
    faults::{Boundary, Checkpoint, Decision, barrier},
    filesystem::{DurableFiles, FaultFiles, RealFiles, ScratchDirectory},
    process::TestChild,
};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;
fn runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

#[test]
fn virtual_time_and_ids_are_explicit_and_reproducible() -> Result {
    let clock = VirtualClock::default();
    let observer = clock.clone();
    clock.advance(Duration::from_secs(3600))?;
    assert_eq!(observer.now()?, Duration::from_secs(3600));
    assert!(
        clock.advance(Duration::MAX).is_err(),
        "Overflow must be visible"
    );
    assert_eq!(clock.now()?, Duration::from_secs(3600));
    let mut ids = SequentialIds::starting_at(42);
    assert_eq!([ids.next_id()?, ids.next_id()?], [42, 43]);
    assert!(SequentialIds::starting_at(u128::MAX).next_id().is_err());
    let mut one = SeededRandom::new(0);
    let mut two = SeededRandom::new(0);
    assert_eq!(one.next_u64(), 0xe220a8397b1dcdaf);
    assert_eq!(two.next_u64(), 0xe220a8397b1dcdaf);
    for _ in 0..100 {
        assert_eq!(one.next_u64(), two.next_u64());
    }
    Ok(())
}

#[test]
fn barrier_pauses_until_explicit_release_and_is_one_shot() -> Result {
    let (mut point, control) = barrier(Boundary::BeforeHeadCommit);
    let clock = VirtualClock::default();
    thread::scope(|scope| -> Result {
        let handle = scope.spawn(move || -> io::Result<()> {
            point.reach(Boundary::AfterArtifactSync)?;
            point.reach(Boundary::BeforeHeadCommit)?;
            assert!(point.reach(Boundary::BeforeHeadCommit).is_err());
            Ok(())
        });
        assert_eq!(control.wait()?, Boundary::BeforeHeadCommit);
        assert!(
            !handle.is_finished(),
            "Worker must still be held at the barrier"
        );
        clock.advance(Duration::from_secs(7200))?;
        assert!(
            !handle.is_finished(),
            "Virtual time does not release publication"
        );
        control.release(Decision::Continue)?;
        handle
            .join()
            .map_err(|_| io::Error::other("Barrier worker panicked"))??;
        Ok(())
    })?;
    Ok(())
}

#[test]
fn dropped_controller_releases_waiter_with_failure() -> Result {
    let (mut point, control) = barrier(Boundary::AfterHeadCommit);
    thread::scope(|scope| -> Result {
        let waiter = scope.spawn(move || point.reach(Boundary::AfterHeadCommit));
        control.wait()?;
        drop(control);
        let result = waiter
            .join()
            .map_err(|_| io::Error::other("Waiter panicked"))?;
        assert_eq!(
            result.err().ok_or("Controller loss must fail")?.kind(),
            io::ErrorKind::BrokenPipe
        );
        Ok(())
    })
}

#[test]
fn disk_full_injection_preserves_existing_bytes() -> Result {
    let directory = ScratchDirectory::new()?;
    let good = directory.path().join("last-good");
    RealFiles.create_synced(&good, b"immutable earlier bytes")?;
    let candidate = directory.path().join("candidate");
    let (checkpoint, control) = barrier(Boundary::BeforeStagingWrite);
    thread::scope(|scope| -> Result {
        let writer = scope.spawn(|| FaultFiles { checkpoint }.create_synced(&candidate, b"new"));
        control.wait()?;
        assert!(!candidate.exists());
        control.release(Decision::NoSpace)?;
        let error = writer
            .join()
            .map_err(|_| io::Error::other("Writer panicked"))?
            .err()
            .ok_or("ENOSPC must propagate")?;
        assert_eq!(error.raw_os_error(), Some(28));
        assert_eq!(fs::read(&good)?, b"immutable earlier bytes");
        assert!(!candidate.exists());
        Ok(())
    })?;
    directory.close()?;
    Ok(())
}

#[test]
fn filesystem_never_overwrites_existing_artifact() -> Result {
    let directory = ScratchDirectory::new()?;
    let path = directory.path().join("existing");
    RealFiles.create_synced(&path, b"old")?;
    assert_eq!(
        RealFiles
            .create_synced(&path, b"new")
            .err()
            .ok_or("No overwrite")?
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(path)?, b"old");
    Ok(())
}

#[test]
#[allow(
    clippy::panic,
    reason = "Intentionally unwind to verify RAII fixture cleanup"
)]
fn scratch_directory_is_cleaned_during_error_and_unwind() -> Result {
    let path;
    {
        let directory = ScratchDirectory::new()?;
        path = directory.path().to_path_buf();
        fs::write(path.join("scratch"), b"temporary")?;
        let result = std::panic::catch_unwind(move || {
            let _owned = directory;
            panic!("synthetic fixture failure");
        });
        assert!(result.is_err());
    }
    assert!(!path.exists());
    Ok(())
}

#[test]
fn actual_sqlite_settings_foreign_keys_and_rollback() -> Result {
    let directory = ScratchDirectory::new()?;
    runtime()?.block_on(async {
        let path = directory.path().join("fixture.sqlite");
        database::prepare(&path).await?;
        let mut db = database::connect(&path).await?;
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut db)
            .await?;
        let sync: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut db)
            .await?;
        assert_eq!(mode, "wal");
        assert_eq!(sync, 2);
        assert!(
            sqlx::query("UPDATE fixture_head SET artifact='missing'")
                .execute(&mut db)
                .await
                .is_err()
        );
        let mut tx = db.begin().await?;
        sqlx::query("UPDATE fixture_head SET artifact='candidate'")
            .execute(&mut *tx)
            .await?;
        tx.rollback().await?;
        db.close().await?;
        assert_eq!(database::head(&path).await?, "last-good");
        Ok(())
    })
}

#[test]
fn cancellation_at_commit_barrier_keeps_last_good_head() -> Result {
    let directory = ScratchDirectory::new()?;
    let path = directory.path().join("fixture.sqlite");
    runtime()?.block_on(database::prepare(&path))?;
    let (mut point, control) = barrier(Boundary::BeforeHeadCommit);
    thread::scope(|scope| -> Result {
        let writer = scope.spawn(
            || -> std::result::Result<(), Box<dyn Error + Send + Sync>> {
                runtime()?.block_on(async {
                    let mut db = database::connect(&path).await?;
                    let mut tx = db.begin().await?;
                    sqlx::query("UPDATE fixture_head SET artifact='candidate'")
                        .execute(&mut *tx)
                        .await?;
                    let result = point.reach(Boundary::BeforeHeadCommit);
                    tx.rollback().await?;
                    db.close().await?;
                    result?;
                    Ok(())
                })
            },
        );
        control.wait()?;
        assert_eq!(runtime()?.block_on(database::head(&path))?, "last-good");
        control.release(Decision::Cancel)?;
        let failure = writer
            .join()
            .map_err(|_| io::Error::other("Transaction worker panicked"))?
            .err()
            .ok_or("Cancellation must fail")?;
        assert_eq!(
            failure.downcast_ref::<io::Error>().map(io::Error::kind),
            Some(io::ErrorKind::Interrupted)
        );
        Ok(())
    })?;
    assert_eq!(runtime()?.block_on(database::head(&path))?, "last-good");
    Ok(())
}

fn child_scenario(command: Option<u8>, kill: bool) -> Result {
    let directory = ScratchDirectory::new()?;
    let path = directory.path().join("fixture.sqlite");
    runtime()?.block_on(database::prepare(&path))?;
    RealFiles.create_synced(&directory.path().join("last-good"), b"original")?;
    let mut child = TestChild::spawn(directory.path())?;
    child.wait_ready()?;
    assert_eq!(runtime()?.block_on(database::head(&path))?, "last-good");
    assert!(directory.path().join("candidate").exists());
    if kill {
        assert!(!child.kill_and_wait()?.success());
    } else {
        match command {
            Some(value) => child.release(value)?,
            None => child.disconnect(),
        }
        assert!(child.finish()?.success());
    }
    let expected = if command == Some(b'C') && !kill {
        "candidate"
    } else {
        "last-good"
    };
    assert_eq!(runtime()?.block_on(database::head(&path))?, expected);
    assert_eq!(fs::read(directory.path().join("last-good"))?, b"original");
    Ok(())
}

#[test]
fn child_can_commit_only_after_parent_release() -> Result {
    child_scenario(Some(b'C'), false)
}
#[test]
fn child_cancellation_rolls_back() -> Result {
    child_scenario(Some(b'X'), false)
}
#[test]
fn coordinator_pipe_loss_rolls_back() -> Result {
    child_scenario(None, false)
}
#[test]
fn killed_child_leaves_recoverable_sqlite_and_last_good_file() -> Result {
    child_scenario(None, true)
}

#[test]
fn child_guard_reaps_on_parent_error() -> Result {
    let directory = ScratchDirectory::new()?;
    let path = directory.path().join("fixture.sqlite");
    runtime()?.block_on(database::prepare(&path))?;
    let fail = || -> Result {
        let child = TestChild::spawn(directory.path())?;
        child.wait_ready()?;
        Err(io::Error::other("Synthetic parent failure").into())
    };
    assert!(fail().is_err());
    // Drop kills and waits synchronously: the DB writer lock must be released.
    runtime()?.block_on(async {
        let mut db = database::connect(&path).await?;
        sqlx::query("UPDATE fixture_head SET artifact='candidate'")
            .execute(&mut db)
            .await?;
        db.close().await
    })?;
    directory.close()?;
    Ok(())
}

#[test]
fn child_failure_before_readiness_fails_closed_and_cleans_up() -> Result {
    let directory = ScratchDirectory::new()?;
    {
        // Deliberately omit the SQL fixture: the child cannot update its head.
        let child = TestChild::spawn(directory.path())?;
        assert!(child.wait_ready().is_err());
    }
    assert!(fs::read_to_string(directory.path().join("child.stderr"))?.contains("fixture_head"));
    directory.close()?;
    Ok(())
}

#[test]
fn coordinator_loss_fault_is_distinct_from_cancellation() {
    assert_eq!(
        Decision::CoordinatorLost
            .result(Boundary::BeforeHeadCommit)
            .map_err(|error| error.kind()),
        Err(io::ErrorKind::BrokenPipe)
    );
}

/// Invoked only in the integration-test executable with an explicit fixture root.
#[test]
fn infrastructure_child() -> Result {
    let Some(root) = std::env::var_os("TRANSFLOW_TEST_CHILD_ROOT") else {
        return Ok(());
    };
    runtime()?.block_on(async {
        let root = Path::new(&root);
        let mut db = database::connect(&root.join("fixture.sqlite")).await?;
        RealFiles.create_synced(&root.join("candidate"), b"candidate bytes")?;
        let mut tx = db.begin().await?;
        sqlx::query("UPDATE fixture_head SET artifact='candidate'")
            .execute(&mut *tx)
            .await?;
        println!("TRANSFLOW_FIXTURE_READY");
        io::stdout().flush()?;
        let mut command = [0];
        let read = io::stdin().read(&mut command)?;
        if read == 1 && command == *b"C" {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        db.close().await?;
        Ok(())
    })
}
