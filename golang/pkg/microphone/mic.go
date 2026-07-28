//go:build darwin && cgo

package microphone

/*
#cgo LDFLAGS: -framework AudioToolbox -framework CoreFoundation
#include <AudioToolbox/AudioToolbox.h>
#include <pthread.h>
#include <string.h>
#include <stdlib.h>
#include <time.h>

// 16 kHz, mono, 16-bit signed = 2 bytes/sample; 2 seconds = 64000 bytes per chunk.
#define MIC_SAMPLE_RATE   16000
#define MIC_CHANNELS      1
#define MIC_BITS          16
#define MIC_BYTES_SAMPLE  (MIC_BITS / 8)
#define MIC_CHUNK_BYTES   (MIC_SAMPLE_RATE * 2 * MIC_CHANNELS * MIC_BYTES_SAMPLE)

static pthread_mutex_t g_mu;
static pthread_cond_t  g_cond;
static unsigned char   g_accum[MIC_CHUNK_BYTES * 3];
static int             g_accum_len;
static unsigned char   g_chunk[MIC_CHUNK_BYTES];
static int             g_chunk_ready;
static int             g_running;
static AudioQueueRef   g_queue;
static AudioQueueBufferRef g_bufs[3];

static void audio_cb(
    void* ud,
    AudioQueueRef aq,
    AudioQueueBufferRef buf,
    const AudioTimeStamp* ts,
    UInt32 np,
    const AudioStreamPacketDescription* pd
) {
    (void)ud; (void)ts; (void)np; (void)pd;
    if (!g_running) return;

    pthread_mutex_lock(&g_mu);

    unsigned char* src = (unsigned char*)buf->mAudioData;
    unsigned int   rem = buf->mAudioDataByteSize;

    while (rem > 0) {
        int space = MIC_CHUNK_BYTES - g_accum_len;
        int n = (int)rem < space ? (int)rem : space;
        memcpy(g_accum + g_accum_len, src, n);
        g_accum_len += n;
        src += n;
        rem -= n;

        if (g_accum_len >= MIC_CHUNK_BYTES) {
            memcpy(g_chunk, g_accum, MIC_CHUNK_BYTES);
            g_chunk_ready = 1;
            g_accum_len -= MIC_CHUNK_BYTES;
            if (g_accum_len > 0) {
                memmove(g_accum, g_accum + MIC_CHUNK_BYTES, g_accum_len);
            }
            pthread_cond_signal(&g_cond);
        }
    }

    pthread_mutex_unlock(&g_mu);

    if (g_running) {
        AudioQueueEnqueueBuffer(aq, buf, 0, NULL);
    }
}

// mic_init starts the audio queue. Returns 0 on success, non-zero OSStatus on failure.
static int mic_init(void) {
    pthread_mutex_init(&g_mu, NULL);
    pthread_cond_init(&g_cond, NULL);

    AudioStreamBasicDescription fmt;
    memset(&fmt, 0, sizeof(fmt));
    fmt.mSampleRate       = MIC_SAMPLE_RATE;
    fmt.mFormatID         = kAudioFormatLinearPCM;
    fmt.mFormatFlags      = kLinearPCMFormatFlagIsSignedInteger | kLinearPCMFormatFlagIsPacked;
    fmt.mBytesPerPacket   = MIC_BYTES_SAMPLE * MIC_CHANNELS;
    fmt.mFramesPerPacket  = 1;
    fmt.mBytesPerFrame    = MIC_BYTES_SAMPLE * MIC_CHANNELS;
    fmt.mChannelsPerFrame = MIC_CHANNELS;
    fmt.mBitsPerChannel   = MIC_BITS;

    OSStatus st = AudioQueueNewInput(&fmt, audio_cb, NULL, NULL, NULL, 0, &g_queue);
    if (st != noErr) return (int)st;

    // 100 ms buffers — three buffers keep the queue filled while we accumulate.
    UInt32 buf_sz = (MIC_SAMPLE_RATE / 10) * MIC_BYTES_SAMPLE * MIC_CHANNELS;
    for (int i = 0; i < 3; i++) {
        st = AudioQueueAllocateBuffer(g_queue, buf_sz, &g_bufs[i]);
        if (st != noErr) { AudioQueueDispose(g_queue, true); g_queue = NULL; return (int)st; }
        st = AudioQueueEnqueueBuffer(g_queue, g_bufs[i], 0, NULL);
        if (st != noErr) { AudioQueueDispose(g_queue, true); g_queue = NULL; return (int)st; }
    }

    g_accum_len   = 0;
    g_chunk_ready = 0;
    g_running     = 1;

    st = AudioQueueStart(g_queue, NULL);
    if (st != noErr) { AudioQueueDispose(g_queue, true); g_queue = NULL; return (int)st; }
    return 0;
}

// mic_read_chunk blocks until a 2-second chunk is ready, timeout_ms elapses, or
// recording stops. Returns 1 (chunk copied to out), 0 (timeout), or -1 (stopped).
static int mic_read_chunk(unsigned char* out, int timeout_ms) {
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    long extra_ns = (long)(timeout_ms % 1000) * 1000000L;
    deadline.tv_sec  += timeout_ms / 1000;
    deadline.tv_nsec += extra_ns;
    if (deadline.tv_nsec >= 1000000000L) {
        deadline.tv_sec++;
        deadline.tv_nsec -= 1000000000L;
    }

    pthread_mutex_lock(&g_mu);
    while (!g_chunk_ready && g_running) {
        int rc = pthread_cond_timedwait(&g_cond, &g_mu, &deadline);
        if (rc != 0) {
            pthread_mutex_unlock(&g_mu);
            return g_running ? 0 : -1;
        }
    }
    if (!g_chunk_ready) {
        pthread_mutex_unlock(&g_mu);
        return -1;
    }
    memcpy(out, g_chunk, MIC_CHUNK_BYTES);
    g_chunk_ready = 0;
    pthread_mutex_unlock(&g_mu);
    return 1;
}

// mic_stop halts recording and releases the audio queue.
static void mic_stop(void) {
    if (!g_queue) return;
    g_running = 0;
    pthread_mutex_lock(&g_mu);
    pthread_cond_broadcast(&g_cond);
    pthread_mutex_unlock(&g_mu);
    AudioQueueStop(g_queue, true);
    AudioQueueDispose(g_queue, true);
    g_queue = NULL;
}
*/
import "C"
import (
	"context"
	"fmt"
	"unsafe"
)

// chunkBytes matches MIC_CHUNK_BYTES: 16000 Hz * 2 s * 1 ch * 2 bytes/sample.
const chunkBytes = 16000 * 2 * 1 * 2

// Start opens the default input device and returns a channel emitting
// 2-second PCM chunks (s16le, 16 kHz, mono). The channel closes when ctx is cancelled.
// Returns an error immediately if the device cannot be opened.
func Start(ctx context.Context) (<-chan []byte, error) {
	if status := C.mic_init(); status != 0 {
		return nil, fmt.Errorf("mic init: AudioQueue error %d", int(status))
	}

	ch := make(chan []byte, 4)
	go func() {
		defer C.mic_stop()
		defer close(ch)

		buf := make([]byte, chunkBytes)
		ptr := (*C.uchar)(unsafe.Pointer(&buf[0]))

		for {
			select {
			case <-ctx.Done():
				return
			default:
			}

			ret := C.mic_read_chunk(ptr, 300) // 300 ms timeout per poll
			switch int(ret) {
			case -1:
				return // queue stopped
			case 0:
				continue // timeout — recheck ctx.Done()
			}

			// chunk ready; deliver a copy to avoid data race on next poll
			chunk := make([]byte, chunkBytes)
			copy(chunk, buf)
			select {
			case <-ctx.Done():
				return
			case ch <- chunk:
			}
		}
	}()

	return ch, nil
}
