import { describe, expect, it } from 'vitest';
import { displayWorkspacePath } from './workspace-path';

describe('工作区路径显示', () => {
  it('显示盘符和网络共享路径，保留目录名和空格', () => {
    expect(displayWorkspacePath(String.raw`\\?\D:\项目 空格\test`)).toBe(String.raw`D:\项目 空格\test`);
    expect(displayWorkspacePath(String.raw`\\?\UNC\server\share\项目`)).toBe(String.raw`\\server\share\项目`);
  });
  it('普通路径和其他设备路径保持原样', () => {
    for (const path of ['D:/project', '/home/user/project', String.raw`\\server\share`, String.raw`\\?\Volume{abc}\project`]) {
      expect(displayWorkspacePath(path)).toBe(path);
    }
  });
});