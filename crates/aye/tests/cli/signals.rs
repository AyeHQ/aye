//! Stop signals: a run stops cleanly, with its workers, whether it is
//! running its tests or still collecting them.

use std::fs;
use std::process::{Command, Stdio};

use aye_test::{as_records, code, project};

use crate::common::{kept, runs_in};

use crate::{aye_command, finished, python};

#[test]
fn sigterm_stops_the_run_and_its_workers() {
    use std::time::{Duration, Instant};

    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_slow.py",
        "import os, time\n\ndef test_slow():\n    with open('pid', 'w') as f:\n        f.write(str(os.getpid()))\n    time.sleep(60)\n",
    )]);
    let before = runs_in(dir.path());
    let child = aye_command(&python, dir.path(), &["--output", "json", "-vvv"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid_file = dir.path().join("pid");
    let started = Instant::now();
    while !pid_file.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the test never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(50));
    let worker = fs::read_to_string(&pid_file).unwrap();
    let signal = |name: &str, pid: &str| {
        Command::new("kill")
            .args([name, pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(signal("-TERM", &child.id().to_string()));
    let stopped = Instant::now();
    let output = child.wait_with_output().unwrap();
    assert!(stopped.elapsed() < Duration::from_secs(20));
    assert_eq!(code(&output), 2);
    let records = kept(dir.path(), &before).unwrap_or_else(|| as_records(&output));
    assert_eq!(records.last().unwrap()["event"], "run_finished");
    assert_eq!(records.last().unwrap()["stopped"], "interrupted");
    // The worker is gone with it.
    let deadline = Instant::now();
    while signal("-0", worker.trim()) {
        assert!(
            deadline.elapsed() < Duration::from_secs(5),
            "worker {worker} outlived aye"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Finding F40: GNU `timeout` sends its signal to aye and then to its own
/// process group, aye included. The two are one stop request: the run still
/// stops cleanly, with `run_finished`, the JUnit report and history.
#[test]
fn a_stop_signal_delivered_twice_still_stops_the_run_cleanly() {
    use std::time::{Duration, Instant};

    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_slow.py",
        "import time\n\ndef test_fast():\n    pass\n\ndef test_slow():\n    open('started', 'w').close()\n    time.sleep(60)\n",
    )]);
    let before = runs_in(dir.path());
    let child = aye_command(
        &python,
        dir.path(),
        &["--output", "json", "-vvv", "--junitxml", "junit.xml"],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
    let started_file = dir.path().join("started");
    let started = Instant::now();
    while !started_file.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the test never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(50));
    let pid = child.id().to_string();
    for signal in ["-TERM", "-TERM"] {
        let sent = Command::new("kill")
            .args([signal, &pid])
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(sent.success());
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(code(&output), 2);
    let records = kept(dir.path(), &before).unwrap_or_else(|| as_records(&output));
    let last = records.last().unwrap();
    assert_eq!(last["event"], "run_finished");
    assert_eq!(last["stopped"], "interrupted");
    assert_eq!(
        finished(&records),
        [(
            "tests/test_slow.py::test_fast".to_string(),
            "passed".to_string()
        )]
    );
    let junit = fs::read_to_string(dir.path().join("junit.xml")).unwrap();
    assert!(junit.contains("test_fast"), "{junit}");
    assert!(dir.path().join(".aye/history.json").is_file());
}

/// Ctrl-C while collecting (here, while a worker imports a file to know its
/// tests) stops aye at once, cleanly: `run_finished` says it was
/// interrupted, exit 2, and the worker is gone.
#[test]
fn a_stop_signal_while_collecting_stops_at_once() {
    use std::time::{Duration, Instant};

    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_slow_import.py",
        "import os, time\n\nimport pytest\n\nwith open('pid', 'w') as f:\n    f.write(str(os.getpid()))\ntime.sleep(60)\nCASES = [n * 2 for n in range(3)]\n\n\n@pytest.mark.parametrize('n', CASES)\ndef test_n(n):\n    pass\n",
    )]);
    let before = runs_in(dir.path());
    let child = aye_command(&python, dir.path(), &["--output", "json", "-vvv"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid_file = dir.path().join("pid");
    let started = Instant::now();
    while !pid_file.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "refinement never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(50));
    let worker = fs::read_to_string(&pid_file).unwrap();
    let signal = |name: &str, pid: &str| {
        Command::new("kill")
            .args([name, pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(signal("-INT", &child.id().to_string()));
    let stopped = Instant::now();
    let output = child.wait_with_output().unwrap();
    assert!(
        stopped.elapsed() < Duration::from_secs(3),
        "took {:?} to stop",
        stopped.elapsed()
    );
    assert_eq!(code(&output), 2);
    let records = kept(dir.path(), &before).unwrap_or_else(|| as_records(&output));
    let last = records.last().unwrap();
    assert_eq!(last["event"], "run_finished");
    assert_eq!(last["stopped"], "interrupted");
    let deadline = Instant::now();
    while signal("-0", worker.trim()) {
        assert!(
            deadline.elapsed() < Duration::from_secs(5),
            "worker {worker} outlived aye"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
