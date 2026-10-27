// SPDX-License-Identifier: MPL-2.0
use libopenzilo::*;
fn calibration() -> ImuCalibration {
    ImuCalibration {
        sensor_type: 0,
        accel_range_g: 16,
        gyro_range_dps: 2000,
        accel_bias_ug: vec![0; 3],
        accel_scale_ppm: vec![1000000; 3],
        gyro_bias_udps: vec![0; 3],
    }
}
fn config() -> ImuConfig {
    ImuConfig {
        sample_rate_hz: 100,
        accel_range_g: 16,
        gyro_range_dps: 2000,
    }
}
fn sample(t: u32) -> ImuSample {
    ImuSample {
        timestamp_ms: t,
        accel_x: 2048,
        accel_y: 0,
        accel_z: 0,
        gyro_x: 16,
        gyro_y: 0,
        gyro_z: 0,
    }
}
fn info() -> DeviceInfo {
    DeviceInfo {
        firmware_version: "synthetic-fw".into(),
        system_time: 0,
        audio_storage_total: 0,
        audio_storage_available: 0,
        battery_percent: 50,
        battery_charging: false,
        sn: String::new(),
        cpuid: "synthetic-identity".into(),
        model: "test".into(),
    }
}
#[test]
fn physical_units_calibration_and_no_double_correction() {
    let b = ImuBatch {
        sequence_start: 0,
        frame_count: 1,
        sample_size: 16,
        samples: vec![sample(1)],
    };
    let mut c = calibration();
    c.accel_bias_ug = vec![10000, 0, 0];
    c.gyro_bias_udps = vec![500000, 0, 0];
    let p = apply_imu_calibration(b, config(), c, true, false).unwrap();
    assert!((p.samples[0].accel_g[0] - 0.99).abs() < 1e-10);
    assert_eq!(p.samples[0].gyro_dps[0], 16.0 * 2000.0 / 32768.0);
    assert_eq!(p.applied_components, ["accel"]);
}
#[test]
fn static_bias_and_six_face_fit_preserve_gyro() {
    let samples = (0..200).map(sample).collect();
    let window = analyze_imu_static_window(samples, config(), true).unwrap();
    assert_eq!(window.accel_g, [1.0, 0.0, 0.0]);
    let initial = fit_imu_gyro_bias(calibration(), window).unwrap();
    assert_eq!(initial.gyro_bias_udps[0], 976562);
    let bias = [0.01, -0.02, 0.005];
    let gain = [1.01, 0.98, 1.02];
    let mut windows = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let mut a = bias.to_vec();
            a[axis] += sign / gain[axis];
            windows.push(ImuStaticWindow {
                accel_g: a,
                gyro_dps: vec![0.0; 3],
                accel_std_g: 0.0,
                gyro_std_dps: 0.0,
                samples: 200,
            });
        }
    }
    let fitted = fit_imu_six_face_calibration(initial.clone(), windows).unwrap();
    assert_eq!(fitted.accel_bias_ug, [10000, -20000, 5000]);
    assert_eq!(fitted.accel_scale_ppm, [1010000, 980000, 1020000]);
    assert_eq!(fitted.gyro_bias_udps, initial.gyro_bias_udps);
}
#[test]
fn saturation_drift_and_residual_are_rejected() {
    let mut samples: Vec<_> = (0..200).map(sample).collect();
    samples[0].accel_x = 32767;
    assert!(analyze_imu_static_window(samples, config(), true).is_err());
    let samples = (0..200)
        .map(|t| {
            let mut s = sample(t);
            s.accel_x += if t < 100 { 0 } else { 10 };
            s
        })
        .collect();
    assert!(analyze_imu_static_window(samples, config(), true).is_err());
    assert!(validate_imu_calibration(
        vec![ImuStaticWindow {
            accel_g: vec![1.02, 0.0, 0.0],
            gyro_dps: vec![0.0; 3],
            accel_std_g: 0.0,
            gyro_std_dps: 0.0,
            samples: 200
        }],
        false,
        CalibrationComponent::Accel
    )
    .is_err());
}
#[test]
fn imu_sequence_and_time_wrap_and_fault_latching() {
    let v = ImuContinuity::new();
    let batch = |seq, t| ImuBatch {
        sequence_start: seq,
        frame_count: 1,
        sample_size: 16,
        samples: vec![sample(t)],
    };
    assert_eq!(
        v.accept(batch(u32::MAX, u32::MAX - 1)).unwrap(),
        [u32::MAX as u64 - 1]
    );
    assert_eq!(v.accept(batch(0, 1)).unwrap(), [u32::MAX as u64 + 2]);
    assert!(v.accept(batch(2, 2)).is_err());
    assert!(v.accept(batch(1, 2)).is_err());
}
#[test]
fn portable_profile_roundtrip_identity_source_and_legacy_boundary() {
    let device = info();
    let profile = CalibrationProfile {
        binding: ProfileBinding::DeviceIdentity,
        device_key: stable_device_key(device.clone()).unwrap(),
        firmware_version: device.firmware_version.clone(),
        model: device.model.clone(),
        calibration: calibration(),
        accel_validated: true,
        gyro_validated: false,
        created_at: "2026-10-27T00:00:00+00:00".into(),
    };
    let json = serialize_calibration_profile(profile.clone()).unwrap();
    assert!(!json.contains("synthetic-identity"));
    assert_eq!(parse_calibration_profile(json.clone()).unwrap(), profile);
    let features = vec![FeatureVersion {
        name: "imu".into(),
        major: 2,
        minor: 0,
    }];
    check_profile_device(
        profile.clone(),
        device.clone(),
        None,
        config(),
        features.clone(),
    )
    .unwrap();
    let mut other = device;
    other.cpuid = "another".into();
    assert!(
        check_profile_device(profile.clone(), other, None, config(), features.clone()).is_err()
    );
    let mut calibrated = features;
    calibrated.push(FeatureVersion {
        name: "imu_calibration".into(),
        major: 1,
        minor: 0,
    });
    assert!(require_raw_imu_source(calibrated).is_err());
    let mut local = profile;
    local.binding = ProfileBinding::LocalLocator;
    let legacy = serialize_calibration_profile(local.clone()).unwrap();
    assert!(legacy.contains("\"schema_version\": 1"));
    assert_eq!(
        parse_calibration_profile(legacy).unwrap().binding,
        ProfileBinding::LocalLocator
    );
    assert!(
        parse_calibration_profile(json.replace("board_adc_counts", "already_corrected")).is_err()
    );
}
#[test]
fn ogg_granules_and_wav_units_are_consistent() {
    let data = build_opus_recording(vec![vec![0xfc, 0xff, 0xfe]; 5], 2).unwrap();
    let summary = opus_recording_info(data.clone()).unwrap();
    assert_eq!(summary.duration_ms, 100);
    assert_eq!(summary.samples_per_channel, 1600);
    let ogg = export_ogg_opus(data, 123).unwrap();
    let mut offset = 0;
    let mut granules = Vec::new();
    while offset < ogg.len() {
        assert_eq!(&ogg[offset..offset + 4], b"OggS");
        let n = ogg[offset + 26] as usize;
        let len: usize = ogg[offset + 27..offset + 27 + n]
            .iter()
            .map(|v| *v as usize)
            .sum();
        granules.push(u64::from_le_bytes(
            ogg[offset + 6..offset + 14].try_into().unwrap(),
        ));
        offset += 27 + n + len;
    }
    assert_eq!(granules, [0, 0, 960, 1920, 2880, 3840, 4800]);
    assert!(export_wav_pcm(
        vec![0; 3],
        PcmFormat {
            sample_rate_hz: 16000,
            channels: 2,
            bit_depth: 16
        }
    )
    .is_err());
    let wav = export_wav_pcm(
        vec![0; 6400],
        PcmFormat {
            sample_rate_hz: 16000,
            channels: 2,
            bit_depth: 16,
        },
    )
    .unwrap();
    assert_eq!(wav.len(), 6444);
    assert_eq!(&wav[..4], b"RIFF");
    let odd = export_wav_pcm(
        vec![128; 3],
        PcmFormat {
            sample_rate_hz: 8000,
            channels: 1,
            bit_depth: 8,
        },
    )
    .unwrap();
    assert_eq!(odd.len(), 48);
    assert_eq!(u32::from_le_bytes(odd[40..44].try_into().unwrap()), 3);
    assert_eq!(odd[47], 0);
    let speex = export_ogg_speex(vec![vec![0; 255]], 16000, 1).unwrap();
    assert!(speex.windows(8).any(|v| v == b"Speex   "));
    assert!(export_ogg_speex(vec![vec![0]], 44100, 1).is_err());
    assert!(matches!(
        decode_opus_pcm(vec![]),
        Err(CoreError::Unavailable { .. })
    ));
}
