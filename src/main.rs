use std::{
    env,
    process::{self, Command},
    thread,
    time::Duration,
};

fn main() -> std::io::Result<()> {
    let mode = env::args().nth(1);

    if mode.as_deref() == Some("worker") {
        println!("Worker started: PID {}", process::id());

        thread::sleep(Duration::from_secs(15));

        println!("Worker finished: PID {}", process::id());

        return Ok(());
    }

    println!("Conductor started: PID {}", process::id());

    let executable = env::current_exe()?;
    let mut children = Vec::new();

    for _ in 0..2 {
        let child = Command::new(&executable).arg("worker").spawn()?;
        println!("Conductor spawned Worker: PID {}", child.id());
        children.push(child);
    }

    for mut child in children {
        let pid = child.id();
        let status = child.wait()?;
        println!("Conductor observed: PID {pid}, status: {status}");
    }

    println!("Conductor finished.");

    Ok(())
}
