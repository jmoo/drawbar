use super::*;

#[test]
fn a_wav_too_large_for_the_browser_says_its_size_and_where_to_open_it() {
    assert_eq!(too_large(WEB_MOST_BYTES, WEB_MOST_BYTES), None);
    assert_eq!(too_large(104 * MB + MB / 2, WEB_MOST_BYTES), None);
    assert_eq!(
        too_large(138_193_000, WEB_MOST_BYTES).as_deref(),
        Some(
            "131 MB: too large for the browser, which opens WAVs of up to 120 MB; open it in the \
             desktop app"
        )
    );
    let just = too_large(120 * MB + MB / 2, WEB_MOST_BYTES).unwrap();
    assert!(just.starts_with("120.5 MB:"), "{just}");
}

fn mono(samples: &[i16]) -> Vec<u8> {
    wav::mono_pcm16(samples, 44_100).unwrap()
}

/// A mono WAV of `encoding` holding `samples`, given on that encoding's own scale
/// (integers for PCM, -1.0..1.0 for float).
fn mono_as(encoding: Encoding, samples: &[f64]) -> Vec<u8> {
    let full = match encoding {
        Encoding::Int { bits } => (1i64 << (bits - 1)) as f64,
        Encoding::Float { .. } => 1.0,
    };
    let audio = Audio {
        rate: 44_100,
        channels: 1,
        samples: samples.iter().map(|s| (s / full) as f32).collect(),
    };
    wav::write(&audio, encoding).unwrap()
}

/// Every integer sample of a PCM WAV, on its own scale.
fn ints_of(bytes: &[u8]) -> Vec<i32> {
    let layout = wav::layout(bytes).unwrap();
    let Encoding::Int { bits } = layout.format.encoding else {
        panic!("not PCM");
    };
    wav::int_samples(bytes, layout.data, bits)
        .unwrap()
        .collect()
}

fn gained(bytes: &[u8], db: &str) -> Result<Vec<u8>, String> {
    apply(bytes, bytes, &[(GAIN.to_string(), db.to_string())]).map(|(out, _)| out)
}

/// Samples clipped scaling `bytes` by `db`.
fn clipped_samples(bytes: &[u8], db: f64) -> usize {
    let layout = wav::layout(bytes).unwrap();
    let mut data = bytes[layout.data].to_vec();
    wav::scale(&mut data, layout.format.encoding, factor(db).unwrap()).unwrap()
}

/// A library holding `bytes` as a saved WAV, and its id.
fn library(bytes: &[u8]) -> (crate::workspace::Workspace, u64) {
    let mut log = crate::log::Log::default();
    let mut workspace = crate::workspace::Workspace::new(crate::testing::context());
    let origin = crate::workspace::Origin::Fresh;
    let id = workspace.ingest("hit.wav".into(), origin, bytes.to_vec(), &mut log);
    (workspace, id)
}

/// The document's state on `bytes`, as it opens on them.
fn opened(bytes: &[u8]) -> State {
    let (workspace, id) = library(bytes);
    State::new(workspace.get(id).unwrap())
}

/// Put `bytes` in the working copy of `id`, unsaved.
fn edited(workspace: &mut crate::workspace::Workspace, id: u64, bytes: Vec<u8>) {
    workspace.replace_bytes(
        id,
        bytes,
        crate::workspace::Cause::User,
        &mut crate::log::Log::default(),
    );
    assert!(workspace.get(id).unwrap().is_unsaved());
}

/// A 16-bit take of decaying 440 Hz notes, one every 2 s from 0.5 s, peaking at each
/// of `peaks` (of full scale) in turn.
fn take(peaks: &[f64]) -> Vec<u8> {
    let rate = 44_100.0;
    let mut samples = vec![0.0; (rate * (0.5 + 2.0 * peaks.len() as f64)) as usize];
    for (k, peak) in peaks.iter().enumerate() {
        let from = (rate * (0.5 + 2.0 * k as f64)) as usize;
        for (i, sample) in samples[from..from + rate as usize].iter_mut().enumerate() {
            let t = i as f64 / rate;
            *sample = peak * (std::f64::consts::TAU * 440.0 * t).sin() * (-t / 0.3).exp();
        }
    }
    let samples: Vec<f64> = samples.iter().map(|s| (s * 32_767.0).round()).collect();
    mono_as(Encoding::Int { bits: 16 }, &samples)
}

#[test]
fn six_db_doubles_a_quiet_sample() {
    let out = gained(&mono(&[1000, -1000, 0]), "6").unwrap();
    let samples = wav::read_pcm16(&out).unwrap().samples;
    // 10^(6/20) = 1.995, so 1000 becomes 1995.
    assert_eq!(samples, [1995, -1995, 0]);
}

#[test]
fn a_gain_clips_at_the_range_of_16_bits() {
    let bytes = mono(&[20_000, -20_000, 100, i16::MIN]);
    assert_eq!(
        wav::read_pcm16(&gained(&bytes, "6.0").unwrap())
            .unwrap()
            .samples,
        [i16::MAX, i16::MIN, 200, i16::MIN]
    );
    assert_eq!(clipped_samples(&bytes, 6.0), 3);
}

#[test]
fn a_second_gain_replaces_the_first_instead_of_stacking_on_it() {
    let bytes = mono(&[1000, 20_000]);
    let twelve = gained(&bytes, "12").unwrap();
    let sets = [
        (GAIN.to_string(), "10".to_string()),
        (GAIN.to_string(), "12".to_string()),
    ];
    let (out, db) = apply(&bytes, &bytes, &sets).unwrap();
    assert_eq!(db, Some(12.0));
    assert!(out == twelve, "10 then 12 dB is 12 dB in all");
    let (back, _) = apply(&bytes, &bytes, &[(GAIN.to_string(), "0".to_string())]).unwrap();
    assert!(
        back == bytes,
        "0 dB is the original, clipping and all undone"
    );
}

#[test]
fn a_gain_changes_only_the_samples_of_the_data_chunk() {
    let mut bytes = mono(&[1000, 2000]);
    let list: &[u8] = b"LIST\x04\x00\x00\x00abcd";
    bytes.splice(36..36, list.iter().copied());
    let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) + list.len() as u32;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    let data = wav::layout(&bytes).unwrap().data;

    let out = gained(&bytes, "-6").unwrap();
    assert_eq!(out.len(), bytes.len());
    let changed: Vec<usize> = (0..bytes.len())
        .filter(|at| out[*at] != bytes[*at])
        .collect();
    assert!(!changed.is_empty());
    assert!(
        changed.iter().all(|at| data.contains(at)),
        "bytes outside {data:?} changed: {changed:?}"
    );
}

#[test]
fn a_gain_the_control_does_not_offer_is_refused() {
    let bytes = mono(&[1000]);
    assert!(gained(&bytes, "25").unwrap_err().contains("runs from"));
    assert!(gained(&bytes, "NaN").is_err());
    assert!(gained(&bytes, "loud").unwrap_err().contains("not a number"));
    let other = apply(&bytes, &bytes, &[("name".to_string(), "x".to_string())]);
    assert!(other.unwrap_err().contains("its cuts"));
}

#[test]
fn a_wav_that_does_not_read_takes_no_gain() {
    let mut bytes = mono(&[1000]);
    bytes[34] = 12;
    assert!(gained(&bytes, "6").unwrap_err().contains("12-bit"));
}

#[test]
fn a_gain_keeps_a_24_bit_file_24_bit_and_clips_at_its_own_range() {
    let top = 8_388_607;
    let bytes = mono_as(
        Encoding::Int { bits: 24 },
        &[1000.0, -1000.0, 5_000_000.0, -8_388_608.0, 1.0],
    );
    let out = gained(&bytes, "6").unwrap();
    assert_eq!(out.len(), bytes.len());
    assert_eq!(
        wav::layout(&out).unwrap().format.encoding,
        Encoding::Int { bits: 24 }
    );
    // 10^(6/20) = 1.995: below 16 bits' step the low sample still keeps its own
    // value, 1 becoming 2, where rounding to 16 bits would have made it 0.
    assert_eq!(ints_of(&out), [1995, -1995, top, -8_388_608, 2]);
    assert_eq!(clipped_samples(&bytes, 6.0), 2);
}

#[test]
fn a_gain_scales_every_integer_width_in_its_own_encoding() {
    for bits in [8, 16, 24, 32] {
        let encoding = Encoding::Int { bits };
        let bytes = mono_as(encoding, &[-50.0, 0.0, 50.0]);
        let out = gained(&bytes, "6").unwrap();
        assert_eq!(wav::layout(&out).unwrap().format.encoding, encoding);
        assert_eq!(ints_of(&out), [-100, 0, 100], "{bits}-bit");
        assert_eq!(clipped_samples(&bytes, 6.0), 0, "{bits}-bit");
    }
}

#[test]
fn a_gain_on_a_float_file_stays_float_and_clips_nothing() {
    for bits in [32, 64] {
        let encoding = Encoding::Float { bits };
        let bytes = mono_as(encoding, &[0.25, -0.75, 0.0]);
        let out = gained(&bytes, "6").unwrap();
        assert_eq!(wav::layout(&out).unwrap().format.encoding, encoding);
        let samples = wav::read(&out).unwrap().samples;
        let factor = 10f32.powf(0.3);
        for (got, was) in samples.iter().zip([0.25f32, -0.75, 0.0]) {
            assert!((got - was * factor).abs() < 1e-6, "{bits}: {got} vs {was}");
        }
        // -0.75 at +6 dB is past full scale: float keeps it.
        assert!(samples[1] < -1.0);
        assert_eq!(clipped_samples(&bytes, 6.0), 0);
        assert_eq!(opened(&bytes).clipping(24.0), Ok(None));
    }
}

#[test]
fn a_24_bit_and_a_float_file_read_as_float_for_the_waveform_and_player() {
    for encoding in [Encoding::Int { bits: 24 }, Encoding::Float { bits: 32 }] {
        let full = match encoding {
            Encoding::Int { .. } => 8_388_608.0,
            Encoding::Float { .. } => 1.0,
        };
        let bytes = mono_as(encoding, &[0.5 * full, -0.25 * full]);
        let state = opened(&bytes);
        assert_eq!(state.audio().unwrap().samples, [0.5, -0.25], "{encoding:?}");
    }
}

#[test]
fn the_clipping_a_gain_would_cause_is_counted_in_strikes() {
    let state = opened(&take(&[0.9, 0.3, 0.05]));
    assert_eq!(state.notes.peaks.len(), 3);
    assert_eq!(state.clipping(0.0), Ok(None));
    assert_eq!(state.clipping(-12.0), Ok(None));
    // 0.9 x 1.995 is past full scale; 0.3 x 3.98 is too; 0.05 x 15.85 is not.
    assert_eq!(state.clipping(6.0), Ok(Some("1 of 3 strikes clips".into())));
    assert_eq!(state.clipping(12.0), Ok(Some("2 of 3 strikes clip".into())));
    assert_eq!(state.clipping(24.0), Ok(Some("2 of 3 strikes clip".into())));
    assert!(state.clipping(25.0).is_err());
}

#[test]
fn a_note_clips_exactly_where_one_of_its_samples_does() {
    let bytes = take(&[0.9, 0.3, 0.05]);
    let state = opened(&bytes);
    let layout = wav::layout(&bytes).unwrap();
    let rate = 44_100;
    for tenths in (0..=240).step_by(5) {
        let db = f64::from(tenths) / 10.0;
        // Each note on its own, at 0.5 + 2k s.
        let mut clipped = 0;
        for k in 0..3 {
            let from = layout.data.start + 2 * (rate / 2 + 2 * rate * k);
            let mut note = bytes[from..from + 2 * rate].to_vec();
            let encoding = layout.format.encoding;
            clipped +=
                usize::from(wav::scale(&mut note, encoding, factor(db).unwrap()).unwrap() > 0);
        }
        assert_eq!(state.notes.clipped(factor(db).unwrap()), clipped, "{db} dB");
    }
}

#[test]
fn a_wav_closed_with_an_unsaved_gain_opens_on_that_gain() {
    let saved = take(&[0.9, 0.3, 0.05]);
    let (mut workspace, id) = library(&saved);
    for db in ["4.0", "-7.5", "12.0"] {
        edited(&mut workspace, id, gained(&saved, db).unwrap());
        let state = State::new(workspace.get(id).unwrap());
        assert_eq!(state.gain, db.parse::<f64>().unwrap(), "{db}");
        assert!(
            state.original() == &saved[..],
            "{db}: gains start from the saved file"
        );
        assert_eq!(
            state.clipping(state.gain),
            opened(&saved).clipping(state.gain),
            "{db}"
        );
    }
    let state = State::new(workspace.get(id).unwrap());
    assert_eq!(state.clipping(12.0), Ok(Some("2 of 3 strikes clip".into())));
}

/// A gain is found again whatever order the edits around it moved the file's chunks
/// into: a category set, then a gain, then a moved cut.
#[test]
fn an_unsaved_gain_among_other_edits_opens_on_that_gain() {
    let saved = marked_take(&[0.9, 0.3]);
    let (mut workspace, id) = library(&saved);
    let set = |path: &str, value: &str| [(path.to_string(), value.to_string())];
    let (kinded, _) = apply(&saved, &saved, &set(KIND, "upright")).unwrap();
    let (gained, _) = apply(&saved, &kinded, &set(GAIN, "3.0")).unwrap();
    let (moved, _) = apply(&saved, &gained, &[cut(hit::Cut::AttackEnd, 51_000)]).unwrap();
    edited(&mut workspace, id, moved);
    let state = State::new(workspace.get(id).unwrap());
    assert_eq!(state.gain, 3.0);
    assert!(
        state.original() == &saved[..],
        "gains start from the saved file"
    );
}

#[test]
fn a_working_copy_that_is_no_gain_of_the_saved_file_is_its_own_original() {
    let saved = take(&[0.9, 0.3, 0.05]);
    let (mut workspace, id) = library(&saved);
    let other = take(&[0.5, 0.5, 0.5]);
    edited(&mut workspace, id, other.clone());
    let state = State::new(workspace.get(id).unwrap());
    assert_eq!(state.gain, 0.0);
    assert!(state.original() == &other[..]);
}

#[test]
fn saving_a_gain_makes_the_saved_file_the_new_original() {
    let saved = take(&[0.9, 0.3, 0.05]);
    let (mut workspace, id) = library(&saved);
    let mut state = State::new(workspace.get(id).unwrap());
    let (out, db) = apply(
        state.original(),
        state.original(),
        &[(GAIN.to_string(), "6.0".into())],
    )
    .unwrap();
    edited(&mut workspace, id, out.clone());
    state.made(workspace.get(id).unwrap().stamp, db.unwrap());
    state.follow(workspace.get(id).unwrap());
    assert_eq!(state.gain, 6.0);

    workspace.mark_saved(id);
    state.follow(workspace.get(id).unwrap());
    assert_eq!(state.gain, 0.0);
    assert!(state.original() == &out[..]);
}

/// A moved cut keeps the audio already read: only the marks are read again, and a
/// gain reads the audio anew.
#[test]
fn a_moved_cut_keeps_the_audio_read() {
    let saved = marked_take(&[0.5, 0.3]);
    let (mut workspace, id) = library(&saved);
    let mut state = State::new(workspace.get(id).unwrap());
    let before = state.audio().unwrap().samples.as_ptr();
    let peak = |state: &State| {
        (state.audio().unwrap().samples.iter()).fold(0f32, |most, v| most.max(v.abs()))
    };
    let was = peak(&state);
    let (out, _) = apply(&saved, &saved, &[cut(hit::Cut::AttackEnd, 51_000)]).unwrap();
    edited(&mut workspace, id, out);
    state.made(workspace.get(id).unwrap().stamp, 0.0);
    state.follow(workspace.get(id).unwrap());
    assert_eq!(
        state.audio().unwrap().samples.as_ptr(),
        before,
        "not decoded again"
    );
    let attack = state.read.marks.as_ref().unwrap().regions[0].frames;
    assert_eq!(attack, 51_000 - 22_050, "the marks are the new ones");

    let (out, db) = apply(&saved, &saved, &[(GAIN.to_string(), "3.0".into())]).unwrap();
    edited(&mut workspace, id, out);
    state.made(workspace.get(id).unwrap().stamp, db.unwrap());
    state.follow(workspace.get(id).unwrap());
    let louder = 20.0 * (peak(&state) / was).log10();
    assert!((louder - 3.0).abs() < 0.1, "a gain reads anew: {louder} dB");
}

/// A save of moved cuts alone makes the saved file the original too, so the next
/// gain starts from it.
#[test]
fn saving_moved_cuts_makes_the_saved_file_the_new_original() {
    let saved = marked_take(&[0.9, 0.3]);
    let (mut workspace, id) = library(&saved);
    let mut state = State::new(workspace.get(id).unwrap());
    let (out, _) = apply(&saved, &saved, &[cut(hit::Cut::AttackEnd, 51_000)]).unwrap();
    edited(&mut workspace, id, out.clone());
    state.follow(workspace.get(id).unwrap());
    assert!(
        state.original() == &saved[..],
        "unsaved, the original stays"
    );

    workspace.mark_saved(id);
    state.follow(workspace.get(id).unwrap());
    assert!(
        state.original() == &out[..],
        "saved, the saved file is the original"
    );
}

/// `take(peaks)` with an attack and release marked on its first note, and a loop.
fn marked_take(peaks: &[f64]) -> Vec<u8> {
    let region = |start: u32, frames: u32, label: &str| wav::Region {
        start,
        frames,
        label: label.into(),
    };
    let marks = wav::Marks {
        regions: vec![
            region(22_050, 30_000, "069-b0-v22"),
            region(52_050, 4000, "069-b2-v22"),
        ],
        loops: vec![wav::Loop::forward(40_000, 41_999)],
    };
    wav::with_marks(&take(peaks), &marks).unwrap()
}

fn cut(cut: hit::Cut, frame: u32) -> (String, String) {
    (cut_path("069-b0-v22", cut), frame.to_string())
}

#[test]
fn a_moved_cut_is_written_to_the_marks_and_changes_no_sample() {
    let bytes = marked_take(&[0.9, 0.3]);
    let sets = [
        cut(hit::Cut::AttackEnd, 51_000),
        cut(hit::Cut::LoopStart, 39_000),
    ];
    let (out, db) = apply(&bytes, &bytes, &sets).unwrap();
    assert_eq!(db, None);
    let marks = wav::read_marks(&out).unwrap();
    assert_eq!(marks.regions[0].frames, 51_000 - 22_050);
    assert_eq!((marks.loops[0].start, marks.loops[0].end), (39_000, 41_999));
    assert_eq!(
        marks.regions[1],
        wav::read_marks(&bytes).unwrap().regions[1]
    );
    let data = wav::layout(&bytes).unwrap().data;
    assert!(out[wav::layout(&out).unwrap().data] == bytes[data]);

    let past = apply(&bytes, &bytes, &[cut(hit::Cut::LoopEnd, 60_000)]);
    assert!(
        past.unwrap_err().contains("loop end"),
        "past the attack's end"
    );
    let none = (format!("{CUT}070-b0-v22/attack-end"), "100".to_string());
    assert!(apply(&bytes, &bytes, &[none]).is_err(), "no such hit");
}

/// A hit, release and loop are added and deleted in the marks, the audio
/// untouched, and each is named for Undo.
#[test]
fn pieces_are_added_and_deleted_in_the_marks() {
    let bytes = marked_take(&[0.9, 0.3]);
    let set = |path: String, value: &str| (path, value.to_string());
    let sets = [
        set(format!("{ADDED}071-b0-v00"), "60000..70000"),
        set(format!("{ADDED}071-b2-v00"), "70000..72000"),
        set(format!("{LOOPED}071-b0-v00"), "64000..65000"),
    ];
    let (out, _) = apply(&bytes, &bytes, &sets).unwrap();
    let marks = wav::read_marks(&out).unwrap();
    assert_eq!(marks.regions.len(), 4);
    assert!(marks
        .loops
        .iter()
        .any(|found| (found.start, found.end) == (64_000, 64_999)));
    let data = wav::layout(&bytes).unwrap().data;
    assert!(out[wav::layout(&out).unwrap().data] == bytes[data]);

    let twice = apply(&out, &out, &[set(format!("{ADDED}071-b0-v00"), "1..2")]);
    assert!(twice.is_err(), "a label the take marks already");
    let outside = apply(&out, &out, &[set(format!("{LOOPED}069-b0-v22"), "10..20")]);
    assert!(outside.is_err(), "a loop outside its attack");

    let delete = |piece: &str| set(format!("{DELETED}069-b0-v22/{piece}"), "");
    let (keyless, _) = apply(&out, &out, &[delete("release")]).unwrap();
    let marks = wav::read_marks(&keyless).unwrap();
    assert!(!marks
        .regions
        .iter()
        .any(|region| region.label == "069-b2-v22"));
    assert_eq!(marks.loops.len(), 2);
    let (loopless, _) = apply(&out, &out, &[delete("loop")]).unwrap();
    assert_eq!(wav::read_marks(&loopless).unwrap().loops.len(), 1);
    let (gone, _) = apply(&out, &out, &[delete("hit")]).unwrap();
    let marks = wav::read_marks(&gone).unwrap();
    assert!(marks
        .regions
        .iter()
        .all(|region| !region.label.starts_with("069")));
    assert_eq!(marks.loops.len(), 1, "the other hit's loop stays");
}

/// What the edits leave the build accepts, as its own check of a take's marks says,
/// and an edit after which the build would refuse the take is refused when made.
#[test]
fn an_edit_the_build_would_refuse_is_refused() {
    let bytes = marked_take(&[0.9, 0.3]);
    let set = |path: String, value: &str| (path, value.to_string());
    let builds = |bytes: &[u8]| {
        let frames = wav::layout(bytes).unwrap().frames();
        let marks = wav::read_marks(bytes).unwrap();
        nord_format::take::build::cuts("take.wav", &marks, frames).map(|cuts| cuts.len())
    };
    let (out, _) = apply(
        &bytes,
        &bytes,
        &[
            set(format!("{ADDED}071-b0-v00"), "60000..70000"),
            set(format!("{ADDED}071-b2-v00"), "70000..72000"),
            set(format!("{LOOPED}071-b0-v00"), "64000..65000"),
            cut(hit::Cut::AttackEnd, 51_000),
        ],
    )
    .unwrap();
    assert_eq!(builds(&out).unwrap(), 4);

    // A release over the loop of its own attack, a fade on a looped attack.
    let refused = [
        set(format!("{ADDED}069-b2-v30"), "41000..52000"),
        set(format!("{ADDED}071-b0-v00-fade"), "68000..70000"),
    ];
    for edit in refused {
        let why = apply(&out, &out, std::slice::from_ref(&edit)).unwrap_err();
        assert!(why.starts_with("the build would refuse this"), "{why}");
    }

    // A take the build refuses already still edits, so it can be mended.
    let broken = apply(&out, &out, &[set(format!("{ADDED}nonsense"), "100..200")]);
    assert!(broken.is_err(), "no stroke");
    // A loop crossing the key-off's start, mended by moving the key-off to hold it.
    let mut marks = wav::read_marks(&out).unwrap();
    marks.loops.push(wav::Loop::forward(51_500, 53_000));
    let crossing = wav::with_marks(&out, &marks).unwrap();
    assert!(builds(&crossing).is_err());
    let (mended, _) = apply(&crossing, &crossing, &[cut(hit::Cut::ReleaseStart, 51_000)]).unwrap();
    assert!(builds(&mended).is_ok());
}

/// A rename relabels every region of the hit, keeps its loop, and refuses a name
/// the take marks already or one whose layer form the note's other layers do not share.
#[test]
fn a_hit_is_renamed_every_region_of_it() {
    let bytes = marked_take(&[0.9, 0.3]);
    let rename = |to: &str| [(format!("{RENAMED}069-b0-v22"), to.to_string())];
    let (out, _) = apply(&bytes, &bytes, &rename("071-b0-v30")).unwrap();
    let marks = wav::read_marks(&out).unwrap();
    let labels: Vec<&str> = marks
        .regions
        .iter()
        .map(|region| region.label.as_str())
        .collect();
    assert_eq!(labels, ["071-b0-v30", "071-b2-v30"]);
    assert_eq!(marks.loops.len(), 1);
    let added = [(format!("{ADDED}071-b0-v30"), "60000..70000".to_string())];
    let (both, _) = apply(&bytes, &bytes, &added).unwrap();
    assert!(apply(&both, &both, &rename("071-b0-v30")).is_err());
    // A name by index beside the note's layer named by value, which the build refuses.
    let why = apply(&both, &both, &rename("071-b0-l01")).unwrap_err();
    assert!(why.starts_with("the build would refuse this"), "{why}");
    assert!(why.contains("some by value"), "{why}");

    assert_eq!(
        hit::named("B4 v30"),
        Ok((71, nord_format::formats::npno::encode::LayerTag::Value(30)))
    );
    assert_eq!(
        hit::named("c4 l01"),
        Ok((60, nord_format::formats::npno::encode::LayerTag::Index(1)))
    );
    assert!(hit::named("B4").is_err() && hit::named("H4 v30").is_err());
}

/// A refused add or rename puts the pick back where it was, so it names no hit the take
/// does not mark; with no menu edit this frame, a refusal leaves the pick alone.
#[test]
fn a_refused_edit_puts_the_pick_back() {
    use nord_format::formats::npno::encode::LayerTag;
    let mut state = opened(&marked_take(&[0.9, 0.3]));
    let was = Some((69, LayerTag::Value(22)));
    state.view.picked = was;
    state.before_edit = Some((state.view.clone(), state.close.clone()));
    state.view.picked = Some((71, LayerTag::Index(1)));
    state.refused();
    assert_eq!(state.view.picked, was);
    state.view.picked = None;
    state.refused();
    assert_eq!(state.view.picked, None, "nothing to put back");
}

/// A resonance hit's fade is renamed with it, and keeps its bank, so the build
/// still finds the stroke it fades.
#[test]
fn a_resonance_hit_s_fade_is_renamed_with_it() {
    let region = |start: u32, frames: u32, label: &str| wav::Region {
        start,
        frames,
        label: label.into(),
    };
    let marks = wav::Marks {
        regions: vec![
            region(22_050, 30_000, "057-b1-v22"),
            region(40_000, 12_050, "057-b1-v22-fade"),
        ],
        loops: Vec::new(),
    };
    let bytes = wav::with_marks(&take(&[0.9, 0.3]), &marks).unwrap();
    let sets = [(format!("{RENAMED}057-b1-v22"), "058-b0-v22".to_string())];
    let (out, _) = apply(&bytes, &bytes, &sets).unwrap();
    let marks = wav::read_marks(&out).unwrap();
    let labels: Vec<&str> = marks.regions.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["058-b1-v22", "058-b1-v22-fade"]);
    let frames = wav::layout(&out).unwrap().frames();
    assert!(nord_format::take::build::cuts("take.wav", &marks, frames).is_ok());
}

#[test]
fn a_gain_keeps_the_cuts_moved_before_it() {
    let saved = marked_take(&[0.9, 0.3]);
    let (moved, _) = apply(&saved, &saved, &[cut(hit::Cut::ReleaseEnd, 58_000)]).unwrap();
    let (out, db) = apply(&saved, &moved, &[(GAIN.to_string(), "-6.0".into())]).unwrap();
    assert_eq!(db, Some(-6.0));
    assert_eq!(
        wav::read_marks(&out).unwrap(),
        wav::read_marks(&moved).unwrap()
    );
    assert!(
        wav::read(&out).unwrap().samples
            == wav::read(&gained(&saved, "-6.0").unwrap()).unwrap().samples
    );

    // Closed unsaved and opened again: the gain is found, and the saved file is still
    // what it is applied to.
    let (mut workspace, id) = library(&saved);
    edited(&mut workspace, id, out);
    let state = State::new(workspace.get(id).unwrap());
    assert_eq!(state.gain, -6.0);
    assert!(state.original() == &saved[..]);
}

#[test]
fn a_file_with_no_onset_is_one_note() {
    let state = opened(&mono(&[20_000, -20_000, 100]));
    assert_eq!(state.notes.peaks, [(-20_000, 20_000)]);
    assert_eq!(state.clipping(6.0), Ok(Some("it clips".into())));
}

#[test]
fn the_header_states_the_rate_channels_format_and_length() {
    let stated = |bytes: &[u8]| facts(&wav::layout(bytes).unwrap());
    assert_eq!(
        stated(&mono(&[0; 44_100])),
        "44100 Hz · mono · 16-bit · 1.000 s"
    );
    let stereo = wav::write(
        &Audio {
            rate: 48_000,
            channels: 2,
            samples: vec![0.0; 144_000],
        },
        Encoding::Float { bits: 32 },
    )
    .unwrap();
    assert_eq!(
        stated(&stereo),
        "48000 Hz · stereo · 32-bit float · 1.500 s"
    );
    let six = wav::write(
        &Audio {
            rate: 44_100,
            channels: 6,
            samples: Vec::new(),
        },
        Encoding::Int { bits: 24 },
    )
    .unwrap();
    assert_eq!(stated(&six), "44100 Hz · 6 channels · 24-bit · 0.000 s");
}

#[test]
fn a_wav_is_a_take_once_it_marks_a_stroke_region() {
    let raw = mono(&[0; 1000]);
    let marked = |label: &str| {
        let region = wav::Region {
            start: 0,
            frames: 500,
            label: label.into(),
        };
        let marks = wav::Marks {
            regions: vec![region],
            loops: Vec::new(),
        };
        wav::with_marks(&raw, &marks).unwrap()
    };
    assert!(!is_take(&raw), "as recorded");
    assert!(!is_take(&marked("intro")), "a region that names no stroke");
    assert!(is_take(&marked("060-b0-v22")));
    assert_eq!(
        crate::strings::kind_word(Kind::Wav, Some(crate::browser::Qualifier::Processed)),
        "WAV (processed)"
    );
}

#[test]
fn only_a_riff_wave_container_is_a_wav_by_its_bytes() {
    assert!(is_wav(&mono(&[0; 8])));
    assert!(!is_wav(b"RIFF"));
    assert!(!is_wav(b"not a wav at all"));
    assert!(!is_wav(&[]));
}

#[test]
fn detect_marks_what_it_found_in_place_of_the_regions() {
    let found = wav::Marks {
        regions: vec![
            wav::Region {
                start: 100,
                frames: 900,
                label: "060-b0-v00".into(),
            },
            wav::Region {
                start: 1_000,
                frames: 500,
                label: "060-b2-v00".into(),
            },
        ],
        loops: vec![wav::Loop::forward(300, 699)],
    };
    let value = detected_value(&found);
    let bare = wav::pcm16(&[0i16; 4_000], 35_002, 1).unwrap();
    let before = wav::Marks {
        regions: vec![wav::Region {
            start: 0,
            frames: 50,
            label: "072-b0-v00".into(),
        }],
        loops: Vec::new(),
    };
    let old = wav::with_marks(&bare, &before).unwrap();
    let sets = [(DETECTED.to_string(), value)];
    let (out, _) = apply(&old, &old, &sets).unwrap();
    assert_eq!(
        wav::read_marks(&out).unwrap(),
        found,
        "the old regions are gone"
    );
    assert!(detected_marks("12 x 060-b0-v00").is_err());
}

/// A take at a rate the detectors cannot take is refused as it opens, not left to
/// panic: 30 Hz, and 7,999 Hz.
#[test]
fn a_take_at_too_low_a_rate_does_not_read() {
    for rate in [30u32, 7_999] {
        let audio = Audio {
            rate,
            channels: 1,
            samples: vec![0.1; rate as usize * 2],
        };
        let bytes = wav::write(&audio, wav::Encoding::Int { bits: 16 }).unwrap();
        let state = opened(&bytes);
        let why = state.read.source.as_ref().unwrap_err();
        assert!(
            why.ends_with("drawbar opens WAVs of 8000 Hz and up"),
            "{why}"
        );
    }
    assert!(opened(&take(&[0.5])).read.source.is_ok());
}

/// Detect asks before it replaces a take's regions, and starts at once on a take
/// that marks none.
#[test]
fn detect_asks_before_replacing_regions() {
    let ctx = egui::Context::default();
    let mut marked = opened(&marked_take(&[0.9, 0.3]));
    marked.start_detect(&ctx);
    assert!(
        marked.detect.asking && !marked.detect.running(),
        "it asks first"
    );
    let mut bare = opened(&take(&[0.9, 0.3]));
    bare.start_detect(&ctx);
    assert!(
        !bare.detect.asking && bare.detect.running(),
        "nothing to replace"
    );
}

/// Detect's result marks the take only while it holds the audio searched, and says
/// the limit it ran with, whatever the field holds since.
#[test]
fn a_detect_result_for_audio_since_changed_is_let_go() {
    let rate = 35_002u32;
    let audio = Audio {
        rate,
        channels: 1,
        samples: (0..rate as usize * 4)
            .map(|i| {
                let t = i as f32 / rate as f32;
                match t {
                    t if t < 1.0 => 0.0,
                    t => 0.5 * (-(t - 1.0)).exp() * (std::f32::consts::TAU * 220.0 * t).sin(),
                }
            })
            .collect(),
    };
    let gained = Audio {
        samples: audio.samples.iter().map(|v| v * 0.5).collect(),
        ..audio.clone()
    };
    let ctx = egui::Context::default();
    let settle = |detect: &mut Detect, now: &Audio| -> Sets {
        loop {
            let mut sets = Sets::new();
            detect.follow(&mut sets, Some(now));
            if !detect.running() {
                return sets;
            }
            std::thread::yield_now();
        }
    };
    let mut detect = Detect::default();
    detect.start(&ctx, &audio);
    assert!(settle(&mut detect, &gained).is_empty(), "nothing marked");
    let said = detect.said().unwrap();
    assert!(said.contains("changed while Detect ran"), "{said}");

    detect.start(&ctx, &audio);
    let sets = settle(&mut detect, &audio);
    assert_eq!(sets.len(), 1, "the take still holds what was searched");

    detect.limit = 200.0;
    detect.start(&ctx, &audio);
    assert!(
        settle(&mut detect, &audio).is_empty(),
        "every note too quiet"
    );
    detect.limit = 70.0;
    let said = detect.said().unwrap();
    assert!(
        said.contains("200 dB or less") && !said.contains("70"),
        "{said}"
    );
}
