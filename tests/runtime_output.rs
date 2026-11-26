use std::sync::Arc;

use datafusion::{
    arrow::array::{Float32Array, RecordBatch, UInt64Array},
    physical_plan::{SendableRecordBatchStream, stream::RecordBatchStreamAdapter},
};
use futures::stream;
use sonicfusion::{FrameDecoder, RenderConfig, frame_schema, write_streaming_wav_at};

#[test]
fn split_chunks_own_sliced_samples_and_keep_absolute_ranges() {
    let config = RenderConfig::default();
    let mut decoder = FrameDecoder::continuous(&config.as_ref().at_frame(101));
    let chunk = {
        let batch = frame_batch(&config, 100, vec![f32::NAN, 0.25, -0.75, 1.0, f32::NAN]);
        decoder.decode_chunk(&batch.slice(1, 3)).unwrap()
    };
    assert_eq!(chunk.frame_range(), 101..104);
    let (empty, all) = chunk.split_at(0);
    assert_eq!(empty.frame_range(), 101..101);
    let (first, rest) = all.split_at(1);
    assert_eq!(first.frame_range(), 101..102);
    assert_eq!(first.samples(), [0.25]);
    assert_eq!(rest.frame_range(), 102..104);
    assert_eq!(rest.samples(), [-0.75, 1.0]);
    let (_, empty) = rest.split_at(rest.len());
    assert_eq!(empty.frame_range(), 104..104);
}

#[tokio::test]
async fn finite_recording_writes_exactly_the_requested_window_without_padding() {
    let config = RenderConfig::builder().frame_count(3).build().unwrap();
    let mut decoder = FrameDecoder::finite_at(&config, 101).unwrap();
    let batches = vec![
        frame_batch(&config, 101, vec![0.25]),
        frame_batch(&config, 102, vec![-0.5, 0.75]),
    ];
    let fake_output: Vec<_> = batches
        .iter()
        .flat_map(|batch| decoder.decode_chunk(batch).unwrap().samples().to_vec())
        .collect();
    decoder.finish().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("window.wav");
    write_streaming_wav_at(&path, batch_stream(&config, batches), &config, 101)
        .await
        .unwrap();
    let actual: Vec<f32> = hound::WavReader::open(path)
        .unwrap()
        .into_samples()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(actual, [0.25, -0.5, 0.75]);
    assert_eq!(actual, fake_output);
}

#[tokio::test]
async fn incomplete_or_failed_publication_leaves_no_partial_files() {
    let config = RenderConfig::builder().frame_count(3).build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("window.wav");
    for (samples, expected) in [
        (vec![0.0; 2], "first missing frame is 103"),
        (vec![0.0; 4], "unexpected extra frame"),
    ] {
        let stream = batch_stream(&config, vec![frame_batch(&config, 101, samples)]);
        let error = write_streaming_wav_at(&path, stream, &config, 101)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(dir.path().read_dir().unwrap().count(), 0);
    }

    let stream = batch_stream(&config, vec![]);
    let error = write_streaming_wav_at(&path, stream, &config, u64::MAX)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("overflows the UInt64 frame domain")
    );
    assert_eq!(dir.path().read_dir().unwrap().count(), 0);

    std::fs::create_dir(&path).unwrap();
    let stream = batch_stream(&config, vec![frame_batch(&config, 101, vec![0.0; 3])]);
    let error = write_streaming_wav_at(&path, stream, &config, 101)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("failed to publish WAV"));
    assert!(path.is_dir());
    assert_eq!(dir.path().read_dir().unwrap().count(), 1);
}

fn frame_batch(config: &RenderConfig, start: u64, samples: Vec<f32>) -> RecordBatch {
    RecordBatch::try_new(
        frame_schema(config),
        vec![
            Arc::new(UInt64Array::from_iter_values(
                start..start + samples.len() as u64,
            )),
            Arc::new(Float32Array::from(samples)),
        ],
    )
    .unwrap()
}

fn batch_stream(config: &RenderConfig, batches: Vec<RecordBatch>) -> SendableRecordBatchStream {
    Box::pin(RecordBatchStreamAdapter::new(
        frame_schema(config),
        stream::iter(batches.into_iter().map(Ok)),
    ))
}
