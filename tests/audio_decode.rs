// SPDX-License-Identifier: MPL-2.0
use libopenzilo::*;
use opus1::{Application, Channels, OpusEncoder, SampleRate};

fn pcm(samples: impl IntoIterator<Item = i16>) -> Vec<u8> {
    samples.into_iter().flat_map(i16::to_le_bytes).collect()
}

fn samples(bytes: &[u8]) -> impl Iterator<Item = i16> + '_ {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(i16::from_le_bytes)
}

#[test]
fn synthetic_stereo_decodes_in_order_without_downmixing() {
    let mut encoder =
        OpusEncoder::new(SampleRate::Hz16000, Channels::Stereo, Application::Audio).unwrap();
    encoder.set_force_channels(Some(Channels::Stereo));
    let packets: Vec<_> = (0..8)
        .map(|frame| {
            let samples: Vec<_> = (0..320)
                .flat_map(|i| {
                    let t = (frame * 320 + i) as f32 / 16_000.0 * std::f32::consts::TAU;
                    [
                        (t.mul_add(375.0, 0.0).sin() * 6000.0) as i16,
                        (t.mul_add(625.0, 0.0).sin() * 3000.0) as i16,
                    ]
                })
                .collect();
            let mut packet = [0; 100];
            let size = encoder.encode_i16(&samples, 320, &mut packet).unwrap();
            packet[..size].to_vec()
        })
        .collect();
    let decoder = RingOpusDecoder::new(2).unwrap();
    assert!(RingOpusDecoder::new(1)
        .unwrap()
        .decode_packet(packets[0].clone())
        .is_err());
    let streaming: Vec<_> = packets
        .iter()
        .flat_map(|p| decoder.decode_packet(p.clone()).unwrap())
        .collect();
    let recording = build_opus_recording(packets, 2).unwrap();
    assert_eq!(streaming, decode_opus_pcm(recording).unwrap());
    assert_eq!(streaming.len(), 8 * 320 * 4);
    assert!(streaming
        .as_chunks::<4>()
        .0
        .iter()
        .skip(320)
        .any(|pair| pair[..2] != pair[2..]));
    assert!(samples(&streaming).any(|s| s.unsigned_abs() > 500));
}

#[test]
fn mono_decode_rejects_truncated_multiframe_without_losing_valid_input() {
    let decoder = RingOpusDecoder::new(1).unwrap();
    assert!(decoder.decode_packet(vec![0x83, 0x48]).is_err());
    let mut encoder =
        OpusEncoder::new(SampleRate::Hz16000, Channels::Mono, Application::Voip).unwrap();
    let mut packet = [0; 100];
    let size = encoder.encode_i16(&[0; 320], 320, &mut packet).unwrap();
    assert_eq!(
        decoder
            .decode_packet(packet[..size].to_vec())
            .unwrap()
            .len(),
        640
    );
}

#[test]
fn denoiser_preserves_aligned_signal_and_attenuates_one_mic_noise() {
    let speech = (0..320).map(|i| if i % 2 == 0 { 4000 } else { -4000 });
    let clean = pcm(speech.clone().flat_map(|s| [s, s]));
    let noise = pcm(speech.clone().flat_map(|s| [s, 0]));
    let result = DualMicDenoiser::new()
        .process([clean.clone(), noise.clone(), noise.clone()].concat())
        .unwrap();
    assert_eq!(&result[..640], pcm(speech));
    assert!(samples(&result[1280..]).all(|s| s.unsigned_abs() == 1000));
    let streaming = DualMicDenoiser::new();
    assert!(streaming.process(vec![0; 1279]).is_err());
    let chunks = [clean, noise.clone(), noise]
        .into_iter()
        .flat_map(|frame| streaming.process(frame).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(chunks, result);
}
