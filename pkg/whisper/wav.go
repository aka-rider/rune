package whisper

import "encoding/binary"

// EncodePCM wraps raw PCM audio data in a RIFF/WAVE container.
// sampleRate: e.g. 16000, channels: e.g. 1, bitsPerSample: e.g. 16.
func EncodePCM(pcm []byte, sampleRate, channels, bitsPerSample int) []byte {
	dataLen := len(pcm)
	buf := make([]byte, 44+dataLen)

	// RIFF chunk
	copy(buf[0:4], "RIFF")
	binary.LittleEndian.PutUint32(buf[4:8], uint32(36+dataLen))
	copy(buf[8:12], "WAVE")

	// fmt sub-chunk
	copy(buf[12:16], "fmt ")
	binary.LittleEndian.PutUint32(buf[16:20], 16) // PCM sub-chunk size
	binary.LittleEndian.PutUint16(buf[20:22], 1)  // audio format: PCM
	binary.LittleEndian.PutUint16(buf[22:24], uint16(channels))
	binary.LittleEndian.PutUint32(buf[24:28], uint32(sampleRate))
	byteRate := sampleRate * channels * bitsPerSample / 8
	binary.LittleEndian.PutUint32(buf[28:32], uint32(byteRate))
	blockAlign := channels * bitsPerSample / 8
	binary.LittleEndian.PutUint16(buf[32:34], uint16(blockAlign))
	binary.LittleEndian.PutUint16(buf[34:36], uint16(bitsPerSample))

	// data sub-chunk
	copy(buf[36:40], "data")
	binary.LittleEndian.PutUint32(buf[40:44], uint32(dataLen))
	copy(buf[44:], pcm)

	return buf
}
