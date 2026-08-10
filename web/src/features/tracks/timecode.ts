/** Frames → `M:SS.t`, the sharpie-friendly tape counter. */
export function timecode(frames: number, sampleRate: number): string {
  if (sampleRate <= 0 || frames < 0) return '0:00.0';
  const totalSeconds = frames / sampleRate;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds - minutes * 60;
  const whole = Math.floor(seconds);
  const tenths = Math.floor((seconds - whole) * 10);
  return `${minutes}:${String(whole).padStart(2, '0')}.${tenths}`;
}
