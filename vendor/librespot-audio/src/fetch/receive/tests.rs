use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::TcpListener,
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize},
        mpsc as blocking,
    },
    thread,
};

use futures_util::{future, stream};
use librespot_core::{SessionConfig, error::ErrorKind};
use tokio::{sync::Semaphore, time::timeout};

use super::*;
use crate::fetch::{AudioFileDownloadStatus, AudioFileStreaming, StreamLoaderController};

#[global_allocator]
static ALLOCATOR: dhat::Alloc = dhat::Alloc;

const FRAME: usize = 16 * 1024;
const BLOCK: usize = 64 * 1024;
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

struct Reply {
    start:      usize,
    requested:  usize,
    advertised: usize,
    sent:       usize,
    pause:      Option<blocking::Receiver<()>>,
    status:     u16,
}

impl Reply {
    fn full(start: usize, length: usize) -> Self {
        Self { start, requested: length, advertised: length, sent: length, pause: None, status: 206 }
    }
}

fn pattern(offset: usize) -> u8 {
    (offset % 251) as u8
}

/// Only a localhost HTTP server is involved. No login, CDN, device or playback.
struct Transfer {
    session:   Session,
    shared:    Arc<AudioFileShared>,
    commands:  mpsc::UnboundedSender<StreamLoaderCommand>,
    completed: oneshot::Receiver<NamedTempFile>,
    fetch:     tokio::task::JoinHandle<AudioFileResult>,
    server:    thread::JoinHandle<()>,
}

impl Transfer {
    fn start(file_size: usize, initial: usize, replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/audio", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for reply in replies {
                let (mut connection, _) = listener.accept().unwrap();
                connection.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
                connection.set_write_timeout(Some(TEST_TIMEOUT)).unwrap();
                let mut reader = BufReader::new(connection.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                assert!(
                    request.to_ascii_lowercase().contains(&format!(
                        "range: bytes={}-{}\r\n",
                        reply.start,
                        reply.start + reply.requested - 1
                    )),
                    "{request}"
                );
                if reply.status == 0 {
                    // Connection drops before response headers.
                    continue;
                }
                write!(
                    connection,
                    "HTTP/1.1 {} Response\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                    reply.status,
                    reply.advertised,
                    reply.start,
                    reply.start + reply.advertised.saturating_sub(1),
                    file_size
                )
                .unwrap();
                let mut sent = 0;
                while sent < reply.sent {
                    let length = FRAME.min(reply.sent - sent);
                    let bytes: Vec<_> = (reply.start + sent..reply.start + sent + length).map(pattern).collect();
                    connection.write_all(&bytes).unwrap();
                    connection.flush().unwrap();
                    sent += length;
                    if sent == FRAME
                        && let Some(pause) = &reply.pause
                    {
                        pause.recv_timeout(TEST_TIMEOUT).unwrap();
                    }
                }
            }
        });
        let session = Session::new(SessionConfig::default(), None);
        let shared = Arc::new(AudioFileShared {
            cdn_url: url.clone(),
            file_size,
            bytes_per_second: 40_000,
            cond: Condvar::new(),
            download_status: Mutex::new(AudioFileDownloadStatus { requested: RangeSet::new(), downloaded: RangeSet::new() }),
            download_streaming: AtomicBool::new(false),
            download_slots: Semaphore::new(1),
            ping_time_ms: AtomicUsize::new(0),
            read_position: AtomicUsize::new(0),
            throughput: AtomicUsize::new(0),
        });
        let output = NamedTempFile::new().unwrap();
        output.as_file().set_len(file_size as u64).unwrap();
        let request = StreamingRequest {
            streamer:         session.spclient().stream_from_cdn(url, 0, initial).unwrap(),
            initial_response: None,
            offset:           0,
            length:           initial,
        };
        let (commands, rx) = mpsc::unbounded_channel();
        let (complete_tx, completed) = oneshot::channel();
        let fetch = tokio::spawn(audio_file_fetch(session.clone(), shared.clone(), request, output, rx, complete_tx));
        Self { session, shared, commands, completed, fetch, server }
    }

    fn controller(&self) -> StreamLoaderController {
        StreamLoaderController {
            channel_tx:    Some(self.commands.clone()),
            stream_shared: Some(self.shared.clone()),
            file_size:     self.shared.file_size,
        }
    }

    async fn wait(&self, start: usize, length: usize) {
        let controller = self.controller();
        timeout(TEST_TIMEOUT, tokio::task::spawn_blocking(move || controller.fetch_blocking(Range::new(start, length))))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    async fn finish(self) {
        let output = timeout(TEST_TIMEOUT, self.completed).await.unwrap().unwrap();
        let bytes = fs::read(output.path()).unwrap();
        assert_eq!(bytes.len(), self.shared.file_size);
        assert!(bytes.iter().copied().enumerate().all(|(offset, byte)| byte == pattern(offset)));
        timeout(TEST_TIMEOUT, self.fetch).await.unwrap().unwrap().unwrap();
        // EOF reads must not issue a further range request after the writer finished.
        let mut reader = AudioFileStreaming {
            read_file:                output.reopen().unwrap(),
            position:                 0,
            stream_loader_command_tx: self.commands,
            shared:                   self.shared,
        };
        assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), bytes.len() as u64);
        assert_eq!(reader.read(&mut [0; 32]).unwrap(), 0);
        self.session.shutdown();
        self.server.join().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefix_is_readable_before_response_ends_and_seek_ranges_complete_the_file() {
    let (release, pause) = blocking::channel();
    let mut first = Reply::full(0, BLOCK);
    first.pause = Some(pause);
    let tail = 6123;
    let transfer = Transfer::start(BLOCK * 2 + tail, BLOCK, vec![first, Reply::full(BLOCK * 2, tail), Reply::full(BLOCK, BLOCK)]);
    // This would time out with whole-response collection: the server is still paused.
    transfer.wait(0, FRAME).await;
    assert!(!transfer.controller().range_available(Range::new(BLOCK - 1, 1)));
    transfer.commands.send(StreamLoaderCommand::Fetch(Range::new(BLOCK * 2, BLOCK))).unwrap();
    release.send(()).unwrap();
    transfer.wait(BLOCK * 2, tail).await;
    transfer.wait(BLOCK, BLOCK).await;
    transfer.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clean_short_response_retries_only_the_missing_suffix() {
    let mut first = Reply::full(0, BLOCK);
    first.advertised = FRAME;
    first.sent = FRAME;
    let transfer = Transfer::start(BLOCK, BLOCK, vec![first, Reply::full(FRAME, BLOCK - FRAME)]);
    transfer.wait(0, BLOCK).await;
    transfer.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mid_body_connection_error_preserves_written_prefix_and_retries_the_suffix() {
    let mut first = Reply::full(0, BLOCK);
    first.sent = FRAME;
    let transfer = Transfer::start(BLOCK, BLOCK, vec![first, Reply::full(FRAME, BLOCK - FRAME)]);
    transfer.wait(0, BLOCK).await;
    transfer.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_error_before_headers_releases_the_range_for_retry() {
    let mut first = Reply::full(0, BLOCK);
    first.status = 0;
    let transfer = Transfer::start(BLOCK, BLOCK, vec![first, Reply::full(0, BLOCK)]);
    transfer.wait(0, BLOCK).await;
    transfer.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_http_response_releases_the_range_for_retry() {
    let mut first = Reply::full(0, BLOCK);
    first.status = 503;
    first.advertised = 0;
    first.sent = 0;
    let transfer = Transfer::start(BLOCK, BLOCK, vec![first, Reply::full(0, BLOCK)]);
    transfer.wait(0, BLOCK).await;
    transfer.finish().await;
}

#[tokio::test]
async fn closing_the_writer_unblocks_a_backpressured_body() {
    let (tx, rx) = mpsc::channel(1);
    tx.send(ReceivedData::Data(PartialFileData { offset: 0, data: Bytes::from_static(b"first") })).await.unwrap();
    let mut offset = 5;
    let mut actual = 0;
    let body = stream::iter([Ok::<_, io::Error>(Bytes::from_static(b"next"))]);
    let receive = receive_body(body, &tx, &mut offset, &mut actual);
    let close = async {
        tokio::task::yield_now().await;
        drop(rx);
    };
    let (result, ()) = future::join(receive, close).await;
    assert_eq!(result.unwrap_err().kind, ErrorKind::Internal);
    assert_eq!((offset, actual), (5, 0));
}

/// Run only this selected test; dhat's counters are process-wide.
/// 320 kbps * 180 s / 8 = 7.2 MB. The old collect+coalesce path holds roughly
/// twice that at peak; the streamed path retains at most the queue and active frames.
#[test]
#[ignore = "select this test with --ignored; allocator counters are process-wide"]
fn transfer_memory_is_bounded_for_a_three_minute_320_kbps_track() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let profiler = dhat::Profiler::builder().testing().build();
    let collected = std::env::var_os("TURNTABLE_AUDIO_COLLECT_BASELINE").is_some();
    let bytes = 320_000 * 180 / 8;
    runtime.block_on(async {
        let chunks = (0..bytes).step_by(FRAME).map(|offset| {
            let length = FRAME.min(bytes - offset);
            Ok::<_, io::Error>(Bytes::from(vec![pattern(offset); length]))
        });
        if collected {
            // The previous production path, retained only as a profiling control.
            let body = http_body_util::StreamBody::new(stream::iter(chunks.map(|data| data.map(hyper::body::Frame::data))));
            let data = BodyExt::collect(body).await.unwrap().to_bytes();
            assert_eq!(data.len(), bytes);
            std::hint::black_box(&data);
        } else {
            let (tx, mut rx) = mpsc::channel(FILE_DATA_CAPACITY);
            let mut offset = 0;
            let mut actual = 0;
            let writer = async {
                let mut written = 0;
                while let Some(ReceivedData::Data(chunk)) = rx.recv().await {
                    assert_eq!(chunk.offset, written);
                    written += chunk.data.len();
                }
                written
            };
            let receiver = async {
                receive_body(stream::iter(chunks), &tx, &mut offset, &mut actual).await.unwrap();
                drop(tx);
            };
            let ((), written) = future::join(receiver, writer).await;
            assert_eq!((offset, actual, written), (bytes, bytes, bytes));
        }
    });
    let stats = dhat::HeapStats::get();
    println!(
        "audio transfer collected={collected}: peak={} B, live={} B, allocated={} B, blocks={}",
        stats.max_bytes, stats.curr_bytes, stats.total_bytes, stats.total_blocks
    );
    if !collected {
        dhat::assert!(stats.max_bytes < 256 * 1024);
    }
    if std::env::var_os("TURNTABLE_AUDIO_MEMORY_PAUSE").is_some() {
        println!("paused audio transfer pid={}", std::process::id());
        io::stdout().flush().unwrap();
        io::stdin().read_line(&mut String::new()).unwrap();
    }
    drop(profiler);
}
