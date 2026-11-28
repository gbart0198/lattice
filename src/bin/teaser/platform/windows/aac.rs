//! The teaser's soundtrack as AAC, with Windows' built-in Media Foundation encoder.

use crate::SR;

/// Encodes interleaved 16-bit stereo PCM to AAC in an .m4a with Windows' built-in Media
/// Foundation encoder (no ffmpeg build or download needed for the audio).
pub fn encode(path: &str, pcm: &[u8]) -> windows::core::Result<()> {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    use windows::core::HSTRING;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let full = std::path::absolute(path).unwrap_or_else(|_| path.into());
        let _ = std::fs::remove_file(&full);
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(full.as_os_str()), None, None)?;

        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SR)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?; // 192 kbps
        let stream = writer.AddStream(&out)?;

        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SR)?;
        input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
        input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, SR * 4)?;
        writer.SetInputMediaType(stream, &input, None)?;
        writer.BeginWriting()?;

        // ~0.1 s chunks; times are in 100 ns units.
        let frames_per_chunk = (SR / 10) as usize;
        let mut frame = 0usize;
        for chunk in pcm.chunks(frames_per_chunk * 4) {
            let buf = MFCreateMemoryBuffer(chunk.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buf.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(chunk.as_ptr(), ptr, chunk.len());
            buf.Unlock()?;
            buf.SetCurrentLength(chunk.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buf)?;
            let frames = chunk.len() / 4;
            sample.SetSampleTime((frame as i64 * 10_000_000) / SR as i64)?;
            sample.SetSampleDuration((frames as i64 * 10_000_000) / SR as i64)?;
            writer.WriteSample(stream, &sample)?;
            frame += frames;
        }
        writer.Finalize()?;
        MFShutdown()?;
    }
    Ok(())
}
