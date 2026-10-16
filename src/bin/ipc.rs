use serde_json::{Value, json};
use std::{
    env,
    io::{self, BufRead, BufReader, Write},
    process::{self, Command, Stdio},
    sync::mpsc,
    thread,
};

#[derive(Clone, Copy)]
enum TileState {
    Pending,
    Assigned(char),
    Stored,
}

enum WorkerEvent {
    Response(usize, String),
    Closed(usize),
    ReadFailed(io::Error),
}

fn draw_board(tiles: &[TileState], completed: usize, worker_pids: &[u32]) -> io::Result<()> {
    print!("\x1b[2J\x1b[H");
    println!("PROCESS WORKSHOP");
    println!("A: PID {} | B: PID {}", worker_pids[0], worker_pids[1]);
    println!();

    for row in tiles.chunks(8) {
        for state in row {
            match state {
                TileState::Pending => print!(". "),
                TileState::Assigned(label) => print!("{label} "),
                TileState::Stored => print!("# "),
            }
        }

        println!();
    }

    println!();
    println!("Completed: {completed}/64");
    println!(". pending | A/B assigned | # stored");
    io::stdout().flush()
}

fn pixel_color(x: u32, y: u32) -> [u8; 3] {
    let r = (x * 255 / 511) as u8;
    let g = (y * 255 / 511) as u8;

    [r, g, 0]
}

fn render_tile(tile_x: u32, tile_y: u32) -> Vec<u8> {
    let mut tile_pixels = vec![0u8; 64 * 64 * 3];

    for local_y in 0..64 {
        for local_x in 0..64 {
            let x = tile_x * 64 + local_x;
            let y = tile_y * 64 + local_y;
            let rgb = pixel_color(x, y);

            let index = ((local_y * 64 + local_x) * 3) as usize;
            tile_pixels[index..index + 3].copy_from_slice(&rgb);
        }
    }

    tile_pixels
}

fn main() -> io::Result<()> {
    let mode = env::args().nth(1);

    // worker
    if mode.as_deref() == Some("worker") {
        eprintln!("Worker ready: PID {}", process::id());
        let delay_ms = env::args()
            .nth(2)
            .expect("Worker delay argument is required")
            .parse::<u64>()
            .expect("Worker delay must be an integer");

        loop {
            let mut request = String::new();

            if io::stdin().read_line(&mut request)? == 0 {
                eprintln!("Worker received EOF: PID {}", process::id());
                break;
            }

            eprintln!("Worker recevied: {}", request.trim_end());

            let message: Value = serde_json::from_str(&request)?;
            let tile_x: u32 = serde_json::from_value(message["tile_x"].clone())?;
            let tile_y: u32 = serde_json::from_value(message["tile_y"].clone())?;
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let tile_pixels = render_tile(tile_x, tile_y);

            eprintln!(
                "Worker calculated tile: ({tile_x}, {tile_y}), {} bytes",
                tile_pixels.len()
            );

            let reply = json!({
                "tile_x": tile_x,
                "tile_y": tile_y,
                "pixels": tile_pixels,
            });
            println!("{reply}");
            io::stdout().flush()?;
        }

        eprintln!("Worker finished: PID {}", process::id());
        return Ok(());
    }

    // conductor
    std::fs::create_dir_all("output")?;
    println!("Conductor started: PID {}", process::id());
    let mut pixels = vec![0u8; 512 * 512 * 3];
    let mut tiles = [TileState::Pending; 64];
    let mut workers = Vec::new();
    let mut worker_pids = Vec::new();
    let worker_labels = ['A', 'B'];
    let mut receiver_handles = Vec::new();
    let (event_tx, event_rx) = mpsc::channel();

    for (worker_index, label) in worker_labels.into_iter().enumerate() {
        let log = std::fs::File::create(format!("output/worker-{label}.log"))?;
        let mut child = Command::new(env::current_exe()?)
            .arg("worker")
            .arg(if label == 'A' { "100" } else { "700" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .spawn()?;

        worker_pids.push(child.id());
        let to_worker = child.stdin.take().expect("Worker stdin is piped");
        let mut from_worker = BufReader::new(child.stdout.take().expect("Worker stdout is piped"));
        let sender = event_tx.clone();

        let receiver_handle = thread::spawn(move || {
            loop {
                let mut response = String::new();
                match from_worker.read_line(&mut response) {
                    Ok(0) => {
                        let _ = sender.send(WorkerEvent::Closed(worker_index));
                        break;
                    }
                    Ok(_) => {
                        if sender
                            .send(WorkerEvent::Response(worker_index, response))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(WorkerEvent::ReadFailed(error));
                        break;
                    }
                }
            }
        });

        receiver_handles.push(receiver_handle);
        workers.push((child, to_worker));
    }
    drop(event_tx);

    // receive
    let mut completed = 0;
    let mut stored_by_worker = [0usize; 2];
    let mut next_tile_id = 0u32;
    draw_board(&tiles, completed, &worker_pids)?;

    // Give each Worker one initial tile.
    for (worker_index, (_, to_worker)) in workers.iter_mut().enumerate() {
        let tile_id = next_tile_id;
        next_tile_id += 1;
        let tile_x = tile_id % 8;
        let tile_y = tile_id / 8;

        let request = json!({ "tile_x": tile_x, "tile_y": tile_y });
        writeln!(to_worker, "{request}")?;
        to_worker.flush()?;

        tiles[tile_id as usize] = TileState::Assigned(worker_labels[worker_index]);
        draw_board(&tiles, completed, &worker_pids)?;
    }

    while completed < 64 {
        let (worker_index, response) = match event_rx.recv().map_err(io::Error::other)? {
            WorkerEvent::Response(worker_index, response) => (worker_index, response),
            WorkerEvent::Closed(worker_index) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!(
                        "Worker {} stdout closed before completion",
                        worker_labels[worker_index]
                    ),
                ));
            }
            WorkerEvent::ReadFailed(error) => return Err(error),
        };

        let message: Value = serde_json::from_str(&response)?;
        let received_tile_x: u32 = serde_json::from_value(message["tile_x"].clone())?;
        let received_tile_y: u32 = serde_json::from_value(message["tile_y"].clone())?;
        let tile_pixels: Vec<u8> = serde_json::from_value(message["pixels"].clone())?;

        for local_y in 0..64u32 {
            let source_index = (local_y * 64 * 3) as usize;
            let x = received_tile_x * 64;
            let y = received_tile_y * 64 + local_y;
            let destination_index = ((y * 512 + x) * 3) as usize;

            pixels[destination_index..destination_index + 64 * 3]
                .copy_from_slice(&tile_pixels[source_index..source_index + 64 * 3]);
        }

        let tile_id = (received_tile_y * 8 + received_tile_x) as usize;
        tiles[tile_id] = TileState::Stored;
        completed += 1;
        stored_by_worker[worker_index] += 1;
        draw_board(&tiles, completed, &worker_pids)?;

        // Give the next tile to the Worker whose response was just stored.
        if next_tile_id < 64 {
            let tile_id = next_tile_id;
            next_tile_id += 1;
            let tile_x = tile_id % 8;
            let tile_y = tile_id / 8;
            let request = json!({ "tile_x": tile_x, "tile_y": tile_y });
            let to_worker = &mut workers[worker_index].1;
            writeln!(to_worker, "{request}")?;
            to_worker.flush()?;

            tiles[tile_id as usize] = TileState::Assigned(worker_labels[worker_index]);
            draw_board(&tiles, completed, &worker_pids)?;
        }
    }

    drop(event_rx);
    for (mut child, to_worker) in workers {
        drop(to_worker);
        let status = child.wait()?;
        println!("Conductor observed: PID {}, status={status}", child.id());
    }
    for receiver_handle in receiver_handles {
        receiver_handle.join().expect("Receiver thread panicked");
    }
    println!(
        "Stored tiles: A={}, B={}",
        stored_by_worker[0], stored_by_worker[1]
    );

    image::save_buffer(
        "output/ipc.png",
        &pixels,
        512,
        512,
        image::ExtendedColorType::Rgb8,
    )
    .expect("Failed to save output/ipc.png");

    println!("Saved output/ipc.png");
    println!("Worker logs: output/worker-A.log, output/worker-B.log");

    Ok(())
}
