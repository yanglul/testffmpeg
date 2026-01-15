use sdl2::video::{Window};
use sdl2::pixels::Color;
use std::time::{ Instant,Duration};
use std::sync::{Arc,Mutex};
use ffmpeg_next::frame::Video;
use ffmpeg_next::format::{Context,input};
use ffmpeg_next::codec::Context as CodecContext;
use ffmpeg_next::software::scaling::Context as scaling_context;
use ffmpeg_next::format::Pixel;
use sdl2::render::Canvas;
use std::thread;
use crossbeam_channel::{bounded, tick, Receiver, select};
use crate::Vstate;


pub struct Videor {
    frame_count: usize,
    current_frame_index: usize,
    frame_buffer: Vec<Video>,
    canvas:Canvas<Window>,
    width: u32,
    height: u32,
    state:Arc<Vstate>,
    ist_index:usize,
    duration:u64,
}

impl Videor {
    pub fn new(width:u32,height:u32,total_frame:usize,video_stream_index:usize,duration:u64) -> anyhow::Result<Self> {
        // 初始化 SDL2
        let sdl_context = sdl2::init().unwrap();
        let video_subsystem = sdl_context.video().unwrap();
        // 获取解码器上下文
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

        
        

        
        Ok(Self {
            frame_count: total_frame,
            current_frame_index: 0,
            canvas:canvas,
            frame_buffer: Vec::new(),
            width: width,
            height: height,
            state:Vstate::Playing.into(),
            ist_index: video_stream_index ,
            duration:duration  , 
        })
    }
    
    pub fn decode_frame(&mut self, packet: &ffmpeg_next::codec::packet::Packet) -> anyhow::Result<()> {
        
        
        Ok(())
    }
    pub fn play(&mut self){
        loop{
            match *self.state {
                Vstate::Playing =>{
                     
                }
                Vstate::Pause|Vstate::Stop=>{
                    break;
                }
            
            }
        
        }
    }
    
}