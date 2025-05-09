// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: MIT

use std::pin::Pin;


use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SizedSample;
use bytemuck::Pod;
use futures::future::OptionFuture;
use futures::FutureExt;
use ringbuf::{traits::*,HeapRb};
use ringbuf::rb::RbRef;
use ringbuf::traits::producer::Producer;
use ringbuf::wrap::caching::Caching;
use std::future::Future;
use std::sync::Arc;
use ringbuf::SharedRb;
use ringbuf::storage::Heap;

use crate::player::ControlCommand;

 
pub fn audio_play<T: Send + Pod + SizedSample + 'static>(
    config: cpal::SupportedStreamConfig,
    device: &cpal::Device,
    mut sample_consumer:  Caching<Arc<SharedRb<Heap<T>>>, false, true>,
) {
    let cpal_stream = device
        .build_output_stream(
            &config.config(),
            move |data, _| {
                let filled = sample_consumer.pop_slice(data);
                data[filled..].fill(T::EQUILIBRIUM);
            },
            move |err| {
                eprintln!("error feeding audio stream to cpal: {}", err);
            },
            None,
        )
        .unwrap();

    cpal_stream.play().unwrap();
}
