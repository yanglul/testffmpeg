use crossbeam_channel::{Receiver, Sender, bounded, select, tick};
use ffmpeg_next::codec::Context as CodecContext;
use ffmpeg_next::decoder::new;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::format::{Context, input};
use ffmpeg_next::frame::Video;
use ffmpeg_next::software::scaling::Context as scaling_context;
use sdl2::pixels::Color;
use sdl2::video::Window;
use std::time::{Duration, Instant};
use std::{
    fmt::Debug,
    sync::{Arc, Mutex,RwLock, mpsc},
    thread,
};

fn main() {
    // let _ = audio::play_mp4(file);
    ffmpeg_next::init().unwrap();

    // 初始化 SDL2
    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();

    let width = 640;
    let height = 480;

    // 创建 SDL2 窗口和画布
    let window = video_subsystem
        .window("FFmpeg + SDL2 Video Player", width, height)
        .position_centered()
        .build()
        .unwrap();
    let mut canvas = window.into_canvas().build().unwrap();
    canvas.set_draw_color(Color::BLACK);
    canvas.clear();
    canvas.present();

    let shared_vec = Arc::new(RwLock::new(Vec::<Vframe>::new()));
    let mut current_frame_index = 0;
    let mut start = Instant::now();
    let (v_tx, v_rx) = mpsc::channel::<Vframe>();
    // let (v_tx, v_rx) = mpsc::channel::<Vframe>(65536);
    // 渲染循环在单独线程
    let cv = Arc::clone(&shared_vec);
    let reader_thread = thread::spawn(move || {
        // 打开输入文件
        let file = &std::env::args().nth(1).expect("Cannot open file.");
        let mut ictx = input(&file).unwrap();
        // 查找视频流
        let video_stream = ictx
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .ok_or(anyhow::anyhow!("No video stream found"))
            .unwrap();

        let audio_stream = ictx
            .streams()
            .best(ffmpeg_next::media::Type::Audio)
            .ok_or(anyhow::anyhow!("No video stream found"))
            .unwrap();

        let audio_stream_index = audio_stream.index();
        let video_stream_index = video_stream.index();
        let duration = ictx.duration(); //总时长
        println!("duration {}", duration);
        let context = CodecContext::from_parameters(video_stream.parameters()).unwrap();
        let total_frams = video_stream.frames();
        println!("total_frams {}", duration / total_frams);
        let fduration = duration / total_frams;
        println!("帧间隔:{}ns", fduration);
        let mut i_iter = ictx.packets();
        // 获取解码器上下文

        let mut decoder_context = context.decoder().video().unwrap();

        let width = decoder_context.width();
        let height = decoder_context.height();
        // 初始化缩放上下文
        let mut scaling_context = ffmpeg_next::software::scaling::Context::get(
            decoder_context.format(),
            width,
            height,
            Pixel::RGB24,
            width,
            height,
            ffmpeg_next::software::scaling::Flags::BILINEAR,
        )
        .unwrap();
        let temp_vec = Arc::clone(&shared_vec);
        loop {
            match i_iter.next() {
                Some((stream, packet)) => {
                    let ist_index = stream.index();
                    if packet.stream() == video_stream_index {
                        // 发送包到解码器
                        decoder_context.send_packet(&packet).unwrap();

                        // 接收解码后的帧
                        let mut decoded_frame = Video::empty();
                        if decoder_context.receive_frame(&mut decoded_frame).is_ok() {
                            // 缩放帧到RGB格式
                            let mut rgb_frame = Video::empty();
                            scaling_context.run(&decoded_frame, &mut rgb_frame).unwrap();

                            // 保存帧数据到缓冲区
                            let vf = Vframe {
                                v: rgb_frame,
                                i: ist_index,
                            };
                            println!("添加帧数");
                            // let mut vec = temp_vec.write().unwrap();
                            // vec.push(vf);
                            v_tx.send(vf).unwrap();

                        }
                    } else if packet.stream() == audio_stream_index {
                    }
                }
                _ => {}
            }
  
        }
    });

    
    loop {
         
        let curent_frame = v_rx.recv();
        match curent_frame {
            Ok(f) => {
                let rgb_frame = &f.v;
                let findex = f.i;
                canvas.clear();
                let texture_creator = canvas.texture_creator();
                let mut texture = texture_creator
                    .create_texture_target(
                        sdl2::pixels::PixelFormatEnum::RGB24,
                        rgb_frame.width() as u32,
                        rgb_frame.height() as u32,
                    )
                    .unwrap();
                texture
                    .update(
                        None,
                        &rgb_frame.data(findex),
                        rgb_frame.stride(findex) as usize,
                    )
                    .unwrap();
                canvas.copy(&texture, None, None).unwrap();
                canvas.present();
                current_frame_index += 1;
                start = Instant::now();
                thread::sleep(Duration::from_millis(33));
            }
            _ => {
                println!("暂无视频")
            }
        }
    }
}

struct VideoPlayer {
    current_frame: Video,
    is_playing: bool,
    last_update: Instant,
}

pub enum Vstate {
    Playing,
    Pause,
    Stop,
}
#[derive(Clone)]
pub struct Vframe {
    v: Video,
    i: usize,
}
