import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useAutoFollow } from './useAutoFollow';

// Exercise native event/timer behavior without adding a DOM test dependency.
const hooks = vi.hoisted(() => ({
  refs: [] as { current: unknown }[],
  effects: [] as { deps: unknown[]; cleanup?: () => void }[],
  queued: [] as (() => void)[],
  refIndex: 0, effectIndex: 0,
}));
vi.mock('react', () => ({
  useRef: (current: unknown) => {
    const index = hooks.refIndex++;
    return hooks.refs[index] ?? (hooks.refs[index] = { current });
  },
  useLayoutEffect: (effect: () => (() => void) | void, deps: unknown[]) => {
    const index = hooks.effectIndex++;
    const previous = hooks.effects[index];
    if (!previous || deps.some((value, i) => value !== previous.deps[i])) {
      hooks.queued.push(() => {
        previous?.cleanup?.();
        hooks.effects[index] = { deps, cleanup: effect() || undefined };
      });
    }
  },
}));

class ScrollBody extends EventTarget {
  scrollHeight = 1000;
  clientHeight = 100;
  children = [];
  private top = 0;
  get scrollTop() { return this.top; }
  set scrollTop(value: number) { this.top = Math.min(value, this.scrollHeight - this.clientHeight); }
  readEarlier() {
    this.scrollTop = 200;
    this.dispatchEvent(new Event('scroll'));
  }
}
const element = new ScrollBody();
function render(streaming: boolean, content: unknown = '内容', open = true) {
  hooks.refIndex = hooks.effectIndex = 0;
  const ref = useAutoFollow(open, content, streaming);
  ref.current = element as unknown as HTMLDivElement;
  hooks.queued.splice(0).forEach(effect => effect());
}
beforeEach(() => {
  vi.useFakeTimers();
  class Observer { observe() {} disconnect() {} }
  vi.stubGlobal('ResizeObserver', Observer);
  vi.stubGlobal('MutationObserver', Observer);
  element.scrollTop = 0;
});
afterEach(() => {
  hooks.effects.forEach(effect => effect.cleanup?.());
  hooks.refs = []; hooks.effects = []; hooks.queued = [];
  vi.useRealTimers(); vi.unstubAllGlobals();
});
describe('对话自动跟随', () => {
  it('已完成内容回滚后超过五秒仍保持阅读位置，包括后续刷新', () => {
    render(false);
    expect(element.scrollTop).toBe(900);
    element.readEarlier();
    vi.advanceTimersByTime(10000);
    render(false, '刷新后的内容');
    expect(element.scrollTop).toBe(200);
    expect(vi.getTimerCount()).toBe(0);
  });
  it('流式输出结束时取消此前的五秒回底计时', () => {
    render(true);
    element.readEarlier();
    vi.advanceTimersByTime(2000);
    render(false, '最终回复');
    vi.advanceTimersByTime(10000);
    expect(element.scrollTop).toBe(200);
  });
  it('仍在输出时保留五秒恢复跟随，新增输出不会重置计时', () => {
    render(true);
    element.readEarlier();
    vi.advanceTimersByTime(4000);
    render(true, '新增内容');
    expect(element.scrollTop).toBe(200);
    vi.advanceTimersByTime(1000);
    expect(element.scrollTop).toBe(900);
  });
  it('手动回到底部可恢复跟随，收起后清理计时器', () => {
    render(false);
    element.readEarlier();
    element.scrollTop = 900;
    element.dispatchEvent(new Event('scroll'));
    element.scrollTop = 800;
    render(false, '新增内容');
    expect(element.scrollTop).toBe(900);
    render(true);
    element.readEarlier();
    render(true, '内容', false);
    expect(vi.getTimerCount()).toBe(0);
  });
});