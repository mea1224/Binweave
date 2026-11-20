import { expect, it } from 'vitest';
import { formatDuration } from './duration';

it('formats elapsed durations across minute and hour boundaries', () => {
  for (const [seconds, text] of [[0, '0 秒'], [59, '59 秒'], [60, '1 分 0 秒'], [3599, '59 分 59 秒'], [3600, '1 小时 0 分'], [7320, '2 小时 2 分'], [-1, '0 秒'], [NaN, '未知']] as const) expect(formatDuration(seconds)).toBe(text);
});