
extern crate ffmpeg_next as ffmpeg;
extern crate sdl2;
use std::env;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ffmpeg::{decoder::{new}, format, frame::{self, Audio, Video}, log, media
};

use ffmpeg_next::software::scaling::{context::Context as ScaleContext, flag::Flags};
use ffmpeg_next::format::Sample as FFmpegSample;
use ffmpeg_next::format::sample::Type as SampleType;
use ffmpeg_next:: codec::  Context as CodecContext;
use ffmpeg_next::util::format::pixel::Pixel;

mod play;
mod player;

use sdl2::video::{Window};
use sdl2::pixels::Color;
use std::time::{ Instant};
extern crate byteorder;
extern crate cpal;
use ffmpeg_next::software::resampling::{context::Context as ResamplingContext};
use cpal::{ SampleFormat};
use ringbuf::{traits::*,HeapRb};
 
trait SampleFormatConversion {
    fn as_ffmpeg_sample(&self) -> FFmpegSample;
}

impl SampleFormatConversion for SampleFormat {
    fn as_ffmpeg_sample(&self) -> FFmpegSample {
        match self {
            Self::I16 => FFmpegSample::I16(SampleType::Packed),
            Self::U16 => {
                panic!("ffmpeg resampler doesn't support u16")
            }, 
            Self::F32 => FFmpegSample::F32(SampleType::Packed),
            _  =>FFmpegSample::F32(SampleType::Packed),

        }
    }
}
 

fn init_cpal() -> (cpal::Device, cpal::SupportedStreamConfig) {
    let device = cpal::default_host()
        .default_output_device()
        .expect("no output device available");

    // Create an output stream for the audio so we can play it
    // NOTE: If system doesn't support the file's sample rate, the program will panic when we try to play,
    //       so we'll need to resample the audio to a supported config
    let supported_config_range = device.supported_output_configs()
        .expect("error querying audio output configs")
        .next()
        .expect("no supported audio config found");

    // Pick the best (highest) sample rate
    (device, supported_config_range.with_max_sample_rate())
}

// Interpret the audio frame's data as packed (alternating channels, 12121212, as opposed to planar 11112222)
pub fn packed<T: frame::audio::Sample>(frame: &frame::Audio) -> &[T] {
    if !frame.is_packed() {
        panic!("data is not packed");
    }

    if !<T as frame::audio::Sample>::is_valid(frame.format(), frame.channels()) {
        panic!("unsupported type");
    }

    unsafe { std::slice::from_raw_parts((*frame.as_ptr()).data[0] as *const T, frame.samples() * frame.channels() as usize) }
}
fn err_fn(err: cpal::StreamError) {
    eprintln!("an error occurred on stream: {}", err);
}

fn main() {
    let input_file = env::args().nth(1).expect("missing input file");
   
    ffmpeg::init().unwrap();
    log::set_level(log::Level::Info);

    let mut ictx = format::input(&input_file).unwrap();
    // let mut octx = format::output(&output_file).unwrap();

    format::context::input::dump(&ictx, 0, Some(&input_file));
    // 查找视频流
    let video_stream_index = ictx
        .streams()
        .best(media::Type::Video)
        .map(|stream| stream.index()).expect("视频索引获取失败");
    

    let video_stream = ictx.stream(video_stream_index).unwrap();
    let codec_ctx = CodecContext::from_parameters(video_stream.parameters()).unwrap();
 

    // 打开解码器
    let mut video_decoder =  codec_ctx.decoder().video().unwrap();

    // 查找音频流
    let audio_stream_index = ictx
        .streams()
        .best(media::Type::Audio)
        .map(|stream| stream.index()).expect("音频索引获取失败");


    let audio_stream = ictx.stream(audio_stream_index).unwrap();
    let audio_codec_ctx = CodecContext::from_parameters(audio_stream.parameters()).unwrap();
    let mut audio_decoder = audio_codec_ctx.decoder().audio().unwrap();

    // 初始化 SDL2
    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();
    // let audio_subsystem = sdl_context.audio().unwrap();


    // 创建 SDL2 窗口和画布
    let window: Window = video_subsystem
        .window("FFmpeg + SDL2 Video Player", 1920, 1080)
        .position_centered()
        .build()
        .unwrap();


    let mut canvas  = window.into_canvas().build().unwrap();
    canvas.set_draw_color(Color::BLACK);
    canvas.clear();
    canvas.present();
    let mut event_pump = sdl_context.event_pump().unwrap();
    let mut i = 0;


    
     

    // Open an audio device
    // let device  = audio_subsystem
    //     .open_queue::<i16, _>(None, &desired_spec)
    //     .unwrap();
    // 创建音频播放器
    // let audio_spec_actual = audio_subsystem
    //     .open_playback(None, &desired_spec,|spec|{
    //         AudioPlayer::new( );
    //     }).expect("msg");

    // Initialize cpal for playing audio
    // Conditionally compile with jack if the feature is specified.
    #[cfg(all(
        any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd"
        ),
        feature = "jack"
    ))]
    // Manually check for flags. Can be passed through cargo with -- e.g.
    // cargo run --release --example beep --features jack -- --jack
    let host = if opt.jack {
        cpal::host_from_id(cpal::available_hosts()
            .into_iter()
            .find(|id| *id == cpal::HostId::Jack)
            .expect(
                "make sure --features jack is specified. only works on OSes where jack is available",
            )).expect("jack host unavailable")
    } else {
        cpal::default_host()
    };

    #[cfg(any(
        not(any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd"
        )),
        not(feature = "jack")
    ))]
    let host = cpal::default_host();    
    let output_device = host.default_output_device();



    let (device, stream_config) = init_cpal();
    println!("audio_decoder.format(){:?}",audio_decoder.format());
    println!("stream_config.sample_format(){:?}",stream_config.sample_format());
    // Set up a resampler for the audio
    let mut resampler = ResamplingContext::get(
        audio_decoder.format(),
        audio_decoder.channel_layout(),
        audio_decoder.rate(),
        
        stream_config.sample_format().as_ffmpeg_sample(),
        audio_decoder.channel_layout(),
        stream_config.sample_rate().0
    ).unwrap();

    // A buffer to hold audio samples
    let buf = HeapRb::<f32>::new(88200);
    let (mut producer, mut consumer) = buf.split();
    let output_data_fn = move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let mut input_fell_behind = false;
        for sample in data {
            *sample = match consumer.try_pop() {
                Some(s) => s,
                None => {
                    input_fell_behind = true;
                    0.0
                }
            };
        }
        if input_fell_behind {
            //填充空白音频todo
        }
    }; 

     
    // let d =  device.build_output_stream(&stream_config.into(),output_data_fn,  err_fn,None).unwrap();
    // Set up the audio output stream
    let audio_stream = match stream_config.sample_format() {
        SampleFormat::F32 => device.build_output_stream(&stream_config.into(),output_data_fn,  err_fn,None),
        SampleFormat::I16 => panic!("i16 output format unimplemented"),
        SampleFormat::U16 => panic!("u16 output format unimplemented"),
        _=> device.build_output_stream(&stream_config.into(),output_data_fn,  err_fn,None),
    }.unwrap();

    // 获取视频帧的缩放上下文
    let mut scaler = ScaleContext::get(
        Pixel::YUV420P,
        video_decoder.width(),
        video_decoder.height(),
        Pixel::RGB24,
        video_decoder.width(),
        video_decoder.height(),
        Flags::BILINEAR,
    ).unwrap();
    
     //加载视频信息
    
    // 播放音视频
    audio_stream.play().unwrap();
    let mut fram_index = 0;
    let mut audio_frame = Audio::empty();
    let mut ist_index =0;
    let mut video_decoded_frame = Video::empty();
    // device.resume();
    let mut i_iter = ictx.packets();
    'main_loop: loop{
        let (stream,mut packet) = i_iter.next().expect("遍历文件失败"); 
        // 读取视频帧
        ist_index = stream.index();
        if packet.stream() == video_stream_index {
            video_decoder.send_packet(&packet).expect("解码视频失败");
            if video_decoder.receive_frame(&mut video_decoded_frame).is_ok() {
                // 对视频帧进行缩放
                let mut rgb_frame = Video::empty();
                scaler.run(&video_decoded_frame, &mut rgb_frame).unwrap();

                // 渲染到 SDL2 画布
                canvas.clear();
                let texture_creator = canvas.texture_creator();
                let mut texture = texture_creator.create_texture_target(
                    sdl2::pixels::PixelFormatEnum::RGB24,
                    rgb_frame.width() as u32,
                    rgb_frame.height() as u32,
                ).unwrap();
                texture.update(None, &rgb_frame.data(ist_index), rgb_frame.stride(ist_index) as usize).unwrap();
                canvas.copy(&texture, None, None).unwrap();
                canvas.present();
             }
        } else if packet.stream() == audio_stream_index {
            packet.rescale_ts(stream.time_base(), audio_decoder.time_base());
            audio_decoder.send_packet(&packet).expect("解码音频失败");
            if audio_decoder.receive_frame(&mut audio_frame).is_ok() {
                // Resample the frame's audio into another frame
                let mut resampled = frame::Audio::empty();
                resampler.run(&audio_frame, &mut resampled).unwrap();

                // DON'T just use resampled.data(0).len() -- it might not be fully populated
                // Grab the right number of bytes based on sample count, bytes per sample, and number of channels.
                let both_channels = packed(&resampled);

                // Sleep until the buffer has enough space for all of the samples
                // (the producer will happily accept a partial write, which we don't want)
                // while producer.capacity().get()< both_channels.len() {
                //     std::thread::sleep(std::time::Duration::from_millis(10));
                // }
                // Buffer the samples for playback
                producer.push_slice(both_channels); 
            }
         }  
        // 控制播放同步
        let now = Instant::now();
    }
    // 播放音频
}

 