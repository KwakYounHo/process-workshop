use std::{
    env,
    process::{self, Command},
};

fn pixel_color(x: u32, y: u32) -> [u8; 3] {
    let r = (x * 255 / 511) as u8;
    let g = (y * 255 / 511) as u8;

    [r, g, 0]
}

fn render_tile(pixels: &mut [u8], tile_x: u32, tile_y: u32) {
    let start_x = tile_x * 64;
    let start_y = tile_y * 64;

    for local_y in 0..64 {
        for local_x in 0..64 {
            let x = start_x + local_x;
            let y = start_y + local_y;
            let color = pixel_color(x, y);
            let index = ((y * 512 + x) * 3) as usize;

            pixels[index..index + 3].copy_from_slice(&color);
        }
    }
}

fn main() -> std::io::Result<()> {
    let mode = env::args().nth(1);
    let mut pixels = vec![0u8; 512 * 512 * 3];
    let index = ((1 * 512 + 66) * 3) as usize;

    if mode.as_deref() == Some("worker") {
        println!("Worker started: PID {}", process::id());

        render_tile(&mut pixels, 1, 0);

        println!(
            "Worker finished: PID {}, RGB={:?}",
            process::id(),
            &pixels[index..index + 3],
        );
        return Ok(());
    }

    println!("Conductor started: PID {}", process::id());
    println!("Conductor before: RGB={:?}", &pixels[index..index + 3]);

    let executable = env::current_exe()?;
    let mut children = Vec::new();

    for _ in 0..4 {
        let child = Command::new(&executable).arg("worker").spawn()?;
        println!("Conductor spawned Worker: PID {}", child.id());
        children.push(child);
    }

    for mut child in children {
        let pid = child.id();
        let status = child.wait()?;
        println!("Conductor observed: PID {pid}, status={status}");
    }

    println!("Conductor after: RGB={:?}", &pixels[index..index + 3]);

    Ok(())
}
