use std::fs;

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

fn main() {
    let mut pixels = vec![0u8; 512 * 512 * 3];

    let mut completed = 0;

    for tile_y in 0..8 {
        for tile_x in 0..8 {
            render_tile(&mut pixels, tile_x, tile_y);
            completed += 1;
            println!("Tile ({tile_x}, {tile_y}) completed: {completed}/64");
        }
    }

    println!("Buffer length: {} bytes", pixels.len());

    for (x, y) in [(0, 0), (511, 0), (0, 511), (511, 511), (2, 1), (66, 1)] {
        let index = ((y * 512 + x) * 3) as usize;
        println!(
            "({x}, {y}): index={index}, RGB={:?}",
            &pixels[index..index + 3]
        );
    }

    fs::create_dir_all("output").expect("Failed to create directory 'output'");

    image::save_buffer(
        "output/output.png",
        &pixels,
        512,
        512,
        image::ExtendedColorType::Rgb8,
    )
    .expect("Failed to save output.png");

    println!("Saved output.png");
}
