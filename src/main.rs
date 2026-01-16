use eframe::{egui,};
use ffmpeg_next as ffmpeg;
use std::sync::{Arc, Mutex,mpsc};
use std::thread;
use ffmpeg_next::codec::Context as CodecContext;
struct FFMpegPlayer {
    texture_handle: Option<egui::TextureHandle>,
    is_playing: bool,
    receiver: mpsc::Receiver<VideoData>,
    sender: mpsc::Sender<VideoData>,
}

struct VideoData {
    width: u32,
    height: u32,
    frame: Vec<u8>,
}

impl Default for FFMpegPlayer {
    fn default() -> Self {
        ffmpeg::init().unwrap();
        let (v_tx, v_rx) = mpsc::channel::<VideoData>();
        Self {
            texture_handle: None,
            is_playing: false,
            receiver:v_rx,
            sender:v_tx,
        }
    }
}

impl eframe::App for FFMpegPlayer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("FFmpeg + EGUI player");
            
            if ui.button("loadVideo").clicked() {
                self.load_video(&"D:\\workspace\\pwork\\decZip\\qyqx.mp4");
            }
            
            // 显示视频
            if let Some(texture) = &self.texture_handle {
                ui.image(texture);
            }
        });
        
        // 更新视频帧
        // let mut video_data = self.video_data.lock().unwrap();
        let result_frame_data = self.receiver.try_recv();
        if self.is_playing && result_frame_data.is_ok() {
            let mut temp_frame = result_frame_data.unwrap();
            let frame_data = &temp_frame.frame;
            
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [temp_frame.width as usize, temp_frame.height as usize],
                frame_data,
            );
            
            if self.texture_handle.is_none() {
                self.texture_handle = Some(ctx.load_texture(
                    "video",
                    color_image,
                    Default::default(),
                ));
            } else {
                self.texture_handle.as_mut().unwrap().set(color_image, Default::default());
            }
            
            ctx.request_repaint();
        }
    }
}

impl FFMpegPlayer {
    fn load_video( &mut self, path: &str) {
        
        thread::spawn(move || {
            if let Ok(mut ictx) = ffmpeg::format::input(&path) {
                let input = ictx
                    .streams()
                    .best(ffmpeg::media::Type::Video)
                    .ok_or(ffmpeg::Error::StreamNotFound)
                    .unwrap();
                
                let stream_index = input.index();
                // let mut decoder = input.codec().decoder().video().unwrap();
                let video_stream = ictx
                .streams()
                .best(ffmpeg_next::media::Type::Video)
                .ok_or(anyhow::anyhow!("No video stream found"))
                .unwrap();
                let context = CodecContext::from_parameters(video_stream.parameters()).unwrap();
                let mut decoder = context.decoder().video().unwrap();
 
                let mut width = 0;
                let mut height = 0;
                
                for (stream, packet) in ictx.packets() {
                    if stream.index() == stream_index {
                        match decoder.send_packet(&packet) {
                            Ok(_) => {
                                let mut decoded = ffmpeg::frame::Video::empty();
                                
                                while decoder.receive_frame(&mut decoded).is_ok() {
                                    let mut scaler = ffmpeg::software::scaling::Context::get(
                                        decoded.format(),
                                        decoded.width(),
                                        decoded.height(),
                                        ffmpeg::format::Pixel::RGBA,
                                        decoded.width(),
                                        decoded.height(),
                                        ffmpeg::software::scaling::Flags::BILINEAR,
                                    ).unwrap();
                                    
                                    let mut rgb_frame = ffmpeg::frame::Video::empty();
                                    scaler.run(&decoded, &mut rgb_frame).unwrap();
                                    
                                    width = rgb_frame.width() as u32;
                                    height = rgb_frame.height() as u32;
                                    
                                    // frames.push(rgb_frame.data(0).to_vec());
                                    let video_data = VideoData {
                                        width,
                                        height,
                                        frame:rgb_frame.data(0).to_vec()
                                    };
                                    self.sender.send(video_data).unwrap();


                                }
                            }
                            Err(e) => eprintln!("解码错误: {}", e),
                        }
                    }
                }
            
                
            }
        });
    }
}


fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Video Player",
        options,
        Box::new(|_cc| Ok(Box::new(FFMpegPlayer::default()))),
    )
}