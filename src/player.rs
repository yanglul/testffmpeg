use std::path::PathBuf;

use ffmpeg_next::frame::Audio;
use ffmpeg_next::frame::Video;
use futures::{future::OptionFuture, FutureExt};
use std::sync::mpsc::Receiver;

use crate::play::audio;
use crate::play::video;

#[derive(Clone, Copy)]
pub enum ControlCommand {
    Play,
    Pause,
}

pub struct Player {
    control_sender: smol::channel::Sender<ControlCommand>,
    demuxer_thread: Option<std::thread::JoinHandle<()>>,
    playing: bool,
    playing_changed_callback: Box<dyn Fn(bool)>,
}


impl Player {
    pub fn start(
        mut input:ffmpeg_next::format::context::input::Input,
        audio_queue: smol::channel::Sender<Audio>,
        video_queue: smol::channel::Sender<Video>,
        video_stream_index:usize,
        audio_stream_index:usize,
        mut video_decoder:ffmpeg_next::codec::decoder::video::Video,
        mut audio_decoder: ffmpeg_next::codec::decoder::audio::Audio,
    ) -> Result<(), anyhow::Error> {
        let mut ist_index = 0;
        let mut i_iter = input.packets();
        
        loop{
            let (stream,mut packet) = i_iter.next().expect("遍历文件失败"); 
            // 读取视频帧
            ist_index = stream.index();
            if packet.stream() == video_stream_index {
                video_decoder.send_packet(&packet).expect("解码视频失败");
                let mut video_decoded_frame = Video::empty();
                if video_decoder.receive_frame(&mut video_decoded_frame).is_ok() {
                    video_queue.send(video_decoded_frame);
                }
            } else if packet.stream() == audio_stream_index {
                packet.rescale_ts(stream.time_base(), audio_decoder.time_base());
                audio_decoder.send_packet(&packet).expect("解码音频失败");
                let mut audio_frame = Audio::empty();
                if audio_decoder.receive_frame(&mut audio_frame).is_ok() {
                    audio_queue.send(audio_frame);
                }
             }  
        }
        Ok(())
    }

    pub fn toggle_pause_playing(&mut self) {
        if self.playing {
            self.playing = false;
            self.control_sender.send_blocking(ControlCommand::Pause).unwrap();
        } else {
            self.playing = true;
            self.control_sender.send_blocking(ControlCommand::Play).unwrap();
        }
        (self.playing_changed_callback)(self.playing);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.control_sender.close();
        if let Some(decoder_thread) = self.demuxer_thread.take() {
            decoder_thread.join().unwrap();
        }
    }
}