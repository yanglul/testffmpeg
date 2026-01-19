use eframe::{egui,};

mod player;


fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Video Player",
        options,
        Box::new(|_cc| Ok(Box::new(FFMpegPlayer::default()))),
    )
}