use std::collections::VecDeque;

use anyhow::{bail, Context, Result};

use super::FinalTranscriptUtterance;
use crate::audio::AUDIO_FRAME_DURATION_MS;

/// Correspondence between queued provider samples and a connection's capture clock.
/// Only successfully queued frames belong here. Muted or dropped frames therefore
/// create capture-time gaps without advancing the provider's sample counter.
#[derive(Default)]
pub(crate) struct TranscriptionClock {
    frames: VecDeque<ClockFrame>,
    first_capture_ms: Option<u64>,
    audio_end_ms: i64,
}

/// One canonical frame's position in the supplied audio and relative capture clocks.
struct ClockFrame {
    audio_start_ms: i64,
    capture_start_ms: i64,
}

impl TranscriptionClock {
    /// Records a frame after its provider input queue accepted it.
    pub(crate) fn record(&mut self, timestamp_ms: u64) -> Result<()> {
        let origin = *self.first_capture_ms.get_or_insert(timestamp_ms);
        let capture_start_ms = i64::try_from(
            timestamp_ms
                .checked_sub(origin)
                .context("capture clock moved backwards")?,
        )?;
        // Browser dispatch jitter can timestamp adjacent frames less than 20 ms apart.
        // Keep sample intervals non-overlapping until the capture clock catches up.
        let capture_start_ms = self.frames.back().map_or(capture_start_ms, |previous| {
            capture_start_ms.max(previous.capture_start_ms + i64::from(AUDIO_FRAME_DURATION_MS))
        });
        self.frames.push_back(ClockFrame {
            audio_start_ms: self.audio_end_ms,
            capture_start_ms,
        });
        self.audio_end_ms = self
            .audio_end_ms
            .checked_add(i64::from(AUDIO_FRAME_DURATION_MS))
            .context("audio clock overflow")?;
        Ok(())
    }

    /// Maps a provider boundary, choosing the appropriate side of a sample-free gap.
    /// Starts at a frame boundary belong to the following frame; endpoints belong
    /// to the preceding frame. Otherwise a word ending before mute could acquire
    /// the entire mute gap as part of its duration.
    fn boundary(&self, time_ms: i64, end: bool) -> Result<i64> {
        if time_ms < 0 || time_ms > self.audio_end_ms {
            bail!("transcript boundary lies outside supplied audio");
        }
        let frame = self
            .frames
            .iter()
            .rev()
            .find(|frame| {
                frame.audio_start_ms < time_ms
                    || (!end && frame.audio_start_ms == time_ms)
                    || (time_ms == 0 && frame.audio_start_ms == 0)
            })
            .context("transcript boundary precedes retained audio")?;
        frame
            .capture_start_ms
            .checked_add(time_ms - frame.audio_start_ms)
            .context("capture boundary overflow")
    }

    /// Converts utterance and token intervals into capture time relative to frame one.
    pub(crate) fn map(
        &self,
        mut utterance: FinalTranscriptUtterance,
    ) -> Result<FinalTranscriptUtterance> {
        utterance.start_time_ms = self.boundary(utterance.start_time_ms, false)?;
        utterance.end_time_ms = self.boundary(utterance.end_time_ms, true)?;
        for token in &mut utterance.tokens {
            let point = token.start_time_ms == token.end_time_ms;
            token.start_time_ms = self.boundary(token.start_time_ms, point)?;
            token.end_time_ms = if point {
                token.start_time_ms
            } else {
                self.boundary(token.end_time_ms, true)?
            };
        }
        if utterance.end_time_ms < utterance.start_time_ms {
            bail!("reversed mapped utterance interval");
        }
        Ok(utterance)
    }

    /// Releases frames before the latest utterance, retaining that utterance for retries.
    pub(crate) fn discard_before(&mut self, provider_start_ms: i64) {
        while self.frames.len() > 1 && self.frames[1].audio_start_ms < provider_start_ms {
            self.frames.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keeps exact gap boundaries distinct for preceding endpoints and following onsets.
    #[test]
    fn missing_samples_do_not_compress_capture_time() {
        let mut clock = TranscriptionClock::default();
        clock.record(900).unwrap();
        clock.record(2_920).unwrap();
        assert_eq!(clock.boundary(20, true).unwrap(), 20);
        assert_eq!(clock.boundary(20, false).unwrap(), 2_020);
        assert_eq!(clock.boundary(30, false).unwrap(), 2_030);
        assert_eq!(clock.boundary(40, true).unwrap(), 2_040);
        assert!(clock.boundary(41, true).is_err());
    }

    /// Full utterance mapping preserves token identity across an omitted-audio gap.
    #[test]
    fn token_mapping_and_retention_preserve_intervals() {
        let mut clock = TranscriptionClock::default();
        clock.record(1000).unwrap();
        clock.record(3020).unwrap();
        let utterance = FinalTranscriptUtterance {
            start_time_ms: 0,
            end_time_ms: 40,
            text: "a b.".into(),
            result_ids: vec![],
            tokens: vec![
                super::super::TranscriptToken {
                    kind: "word".into(),
                    text: "a".into(),
                    start_time_ms: 0,
                    end_time_ms: 20,
                    confidence: None,
                },
                super::super::TranscriptToken {
                    kind: "punctuation".into(),
                    text: ".".into(),
                    start_time_ms: 20,
                    end_time_ms: 20,
                    confidence: None,
                },
                super::super::TranscriptToken {
                    kind: "word".into(),
                    text: "b".into(),
                    start_time_ms: 20,
                    end_time_ms: 40,
                    confidence: None,
                },
            ],
        };
        let mapped = clock.map(utterance.clone()).unwrap();
        assert_eq!(mapped.start_time_ms, 0);
        assert_eq!(mapped.end_time_ms, 2040);
        assert_eq!(mapped.tokens[0].end_time_ms, 20);
        assert_eq!(mapped.tokens[1].start_time_ms, 20);
        assert_eq!(mapped.tokens[1].end_time_ms, 20);
        assert_eq!(mapped.tokens[2].start_time_ms, 2020);
        clock.discard_before(20);
        assert_eq!(clock.map(utterance).unwrap().tokens[2].start_time_ms, 2020);
    }

    /// Closely batched browser messages cannot reverse word boundaries.
    #[test]
    fn capture_dispatch_jitter_keeps_intervals_monotonic() {
        let mut clock = TranscriptionClock::default();
        clock.record(100).unwrap();
        clock.record(100).unwrap();
        clock.record(121).unwrap();
        assert_eq!(clock.boundary(20, false).unwrap(), 20);
        assert_eq!(clock.boundary(40, false).unwrap(), 40);
    }

    /// Each replacement connection starts its own sample/capture correspondence.
    #[test]
    fn replacement_stream_has_a_fresh_origin() {
        let mut clock = TranscriptionClock::default();
        clock.record(50_000).unwrap();
        assert_eq!(clock.boundary(0, false).unwrap(), 0);
        assert_eq!(clock.boundary(20, true).unwrap(), 20);
    }
}
