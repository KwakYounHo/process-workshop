use eframe::egui;
use serde_json::{Value, json};
use std::{
    env,
    io::{self, BufRead, BufReader, Write},
    process::{self, Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread::{self, JoinHandle},
    time::Duration,
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

struct Workshop {
    workers: Vec<(Child, ChildStdin)>,
    receiver_handles: Vec<JoinHandle<()>>,
    event_rx: Receiver<WorkerEvent>,
    pixels: Vec<u8>,
    tiles: [TileState; 64],
    texture: Option<egui::TextureHandle>,
    completed: usize,
    stored_by_worker: [usize; 2],
    next_tile_id: u32,
    started: bool,
    error: Option<String>,
}

impl Workshop {
    fn new() -> io::Result<Self> {
        std::fs::create_dir_all("output")?;
        let worker_exe = env::current_exe()?.with_file_name("ipc");
        let (event_tx, event_rx) = mpsc::channel();
        let mut app = Self {
            workers: Vec::new(),
            receiver_handles: Vec::new(),
            event_rx,
            pixels: vec![0u8; 512 * 512 * 3],
            tiles: [TileState::Pending; 64],
            texture: None,
            completed: 0,
            stored_by_worker: [0; 2],
            next_tile_id: 0,
            started: false,
            error: None,
        };

        for (worker_index, label) in ['A', 'B'].into_iter().enumerate() {
            let log = std::fs::File::create(format!("output/viewer-worker-{label}.log"))?;
            let mut child = Command::new(&worker_exe)
                .arg("worker")
                .arg(if label == 'A' { "100" } else { "700" })
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::from(log))
                .spawn()?;
            let to_worker = child.stdin.take().expect("Worker stdin is piped");
            let mut from_worker =
                BufReader::new(child.stdout.take().expect("Worker stdout is piped"));
            let sender = event_tx.clone();
            let handle = thread::spawn(move || {
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
            app.workers.push((child, to_worker));
            app.receiver_handles.push(handle);
        }
        drop(event_tx);
        Ok(app)
    }

    fn assign_next_tile(&mut self, worker_index: usize) -> io::Result<()> {
        if self.next_tile_id >= 64 {
            return Ok(());
        }
        let tile_id = self.next_tile_id;
        let request = json!({ "tile_x": tile_id % 8, "tile_y": tile_id / 8 });
        let to_worker = &mut self.workers[worker_index].1;
        writeln!(to_worker, "{request}")?;
        to_worker.flush()?;
        self.tiles[tile_id as usize] = TileState::Assigned(['A', 'B'][worker_index]);
        self.next_tile_id += 1;
        Ok(())
    }

    fn start(&mut self) -> io::Result<()> {
        for worker_index in 0..self.workers.len() {
            self.assign_next_tile(worker_index)?;
        }

        self.started = true;
        Ok(())
    }

    fn store_response(&mut self, worker_index: usize, response: &str) -> io::Result<()> {
        let message: Value = serde_json::from_str(response)?;
        let tile_x: u32 = serde_json::from_value(message["tile_x"].clone())?;
        let tile_y: u32 = serde_json::from_value(message["tile_y"].clone())?;
        let tile_pixels: Vec<u8> = serde_json::from_value(message["pixels"].clone())?;
        for local_y in 0..64u32 {
            let source_index = (local_y * 64 * 3) as usize;
            let x = tile_x * 64;
            let y = tile_y * 64 + local_y;
            let destination_index = ((y * 512 + x) * 3) as usize;
            self.pixels[destination_index..destination_index + 64 * 3]
                .copy_from_slice(&tile_pixels[source_index..source_index + 64 * 3]);
        }
        self.tiles[(tile_y * 8 + tile_x) as usize] = TileState::Stored;
        self.completed += 1;
        self.stored_by_worker[worker_index] += 1;
        self.assign_next_tile(worker_index)?;
        if self.completed == 64 {
            image::save_buffer(
                "output/viewer.png",
                &self.pixels,
                512,
                512,
                image::ExtendedColorType::Rgb8,
            )
            .map_err(io::Error::other)?;
            println!(
                "Stored tiles: A={}, B={}",
                self.stored_by_worker[0], self.stored_by_worker[1]
            );
            println!("Saved output/viewer.png");
        }
        Ok(())
    }

    // Process at most one event per GUI update, then return to drawing.
    fn poll_event(&mut self) -> io::Result<bool> {
        match self.event_rx.try_recv() {
            Ok(WorkerEvent::Response(worker_index, response)) => {
                self.store_response(worker_index, &response)?;
                Ok(true)
            }
            Ok(WorkerEvent::Closed(worker_index)) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "Worker {} stdout closed before completion",
                    ['A', 'B'][worker_index]
                ),
            )),
            Ok(WorkerEvent::ReadFailed(error)) => Err(error),
            Err(TryRecvError::Empty) => Ok(false),
            Err(TryRecvError::Disconnected) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Worker event channel disconnected",
            )),
        }
    }
}

impl eframe::App for Workshop {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut changed = false;
        if self.started && self.completed < 64 && self.error.is_none() {
            match self.poll_event() {
                Ok(received) => changed = received,
                Err(error) => self.error = Some(error.to_string()),
            }
        }

        if self.texture.is_none() || changed {
            let image = egui::ColorImage::from_rgb([512, 512], &self.pixels);
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::NEAREST);
            } else {
                self.texture = Some(ctx.load_texture(
                    "process-workshop",
                    image,
                    egui::TextureOptions::NEAREST,
                ));
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("PROCESS WORKSHOP");
            ui.label(format!("Conductor PID: {}", process::id()));
            ui.label(format!(
                "A: PID {} | {} tiles\tB: PID {} | {} tiles",
                self.workers[0].0.id(),
                self.stored_by_worker[0],
                self.workers[1].0.id(),
                self.stored_by_worker[1]
            ));
            if ui
                .add_enabled(
                    !self.started && self.error.is_none(),
                    egui::Button::new("Start rendering"),
                )
                .clicked()
            {
                if let Err(error) = self.start() {
                    self.error = Some(error.to_string());
                }
            }
            ui.add(
                egui::ProgressBar::new(self.completed as f32 / 64.0)
                    .text(format!("Completed: {}/64", self.completed)),
            );
            ui.label("Gray: pending | A/B: assigned | Image: stored");
            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            } else if self.completed == 64 {
                ui.label("Saved output/viewer.png");
            }

            let side = ui
                .available_width()
                .min(ui.available_height())
                .max(1.0)
                .min(512.0);
            let size = egui::vec2(side, side);
            let texture = self.texture.as_ref().expect("Texture initialized above");
            let image = ui.add(egui::Image::new((texture.id(), size)).fit_to_exact_size(size));
            for (tile_id, state) in self.tiles.iter().enumerate() {
                let (fill, label) = match state {
                    TileState::Pending => (egui::Color32::from_rgb(44, 48, 56), None),
                    TileState::Assigned(label) => (
                        if *label == 'A' {
                            egui::Color32::from_rgb(48, 98, 170)
                        } else {
                            egui::Color32::from_rgb(160, 92, 38)
                        },
                        Some(*label),
                    ),
                    TileState::Stored => continue,
                };
                let tile_size = image.rect.size() / 8.0;
                let min = image.rect.min
                    + egui::vec2(
                        (tile_id % 8) as f32 * tile_size.x,
                        (tile_id / 8) as f32 * tile_size.y,
                    );
                let rect = egui::Rect::from_min_size(min, tile_size);
                ui.painter().rect_filled(rect, 0.0, fill);
                if let Some(label) = label {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::monospace(18.0),
                        egui::Color32::WHITE,
                    );
                }
            }
        });

        if self.started && self.completed < 64 && self.error.is_none() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

impl Drop for Workshop {
    fn drop(&mut self) {
        for (mut child, to_worker) in self.workers.drain(..) {
            drop(to_worker);
            match child.wait() {
                Ok(status) => println!("Conductor observed: PID {}, status={status}", child.id()),
                Err(error) => eprintln!("Worker wait failed: {error}"),
            }
        }
        for handle in self.receiver_handles.drain(..) {
            if handle.join().is_err() {
                eprintln!("Receiver thread panicked");
            }
        }
    }
}
fn main() -> io::Result<()> {
    let app = Workshop::new()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([580.0, 710.0])
            .with_min_inner_size([340.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Process Workshop",
        options,
        Box::new(move |_cc| Ok(Box::new(app))),
    )
    .map_err(|error| io::Error::other(error.to_string()))
}
