import { useLayoutEffect, useRef } from 'react';

// Only user scrolling pauses follow; incoming output never resets the idle clock.
export function useAutoFollow(open: boolean, content: unknown, streaming: boolean) {
  const body = useRef<HTMLDivElement>(null);
  const paused = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const programmaticTop = useRef(0);
  const isStreaming = useRef(streaming);
  useLayoutEffect(() => {
    isStreaming.current = streaming;
    if (!streaming) clearTimeout(timer.current);
  }, [streaming]);
  useLayoutEffect(() => {
    const element = body.current;
    if (!element || !open) return;
    const follow = () => {
      if (paused.current) return;
      element.scrollTop = element.scrollHeight;
      programmaticTop.current = element.scrollTop;
    };
    const atBottom = () => element.scrollHeight - element.clientHeight - element.scrollTop <= 2;
    const resume = () => {
      clearTimeout(timer.current);
      paused.current = false;
      follow();
    };
    const pause = () => {
      paused.current = true;
      clearTimeout(timer.current);
      if (isStreaming.current) timer.current = setTimeout(() => {
        if (isStreaming.current) resume();
      }, 5000);
    };
    const scroll = () => {
      if (atBottom()) { resume(); return; }
      if (Math.abs(element.scrollTop - programmaticTop.current) > 1) pause();
    };
    const wheel = (e: WheelEvent) => {
      if (e.deltaY >= 0 && atBottom()) resume();
      else pause();
    };
    const key = (e: KeyboardEvent) => {
      if (['ArrowDown', 'PageDown', 'End', ' '].includes(e.key) && atBottom()) { resume(); return; }
      if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(e.key)) pause();
    };
    element.addEventListener('scroll', scroll);
    element.addEventListener('wheel', wheel, { passive: true });
    element.addEventListener('touchmove', pause, { passive: true });
    element.addEventListener('keydown', key);
    // Expanding code is also a deliberate reading action.
    element.addEventListener('click', pause);
    follow();
    const observer = new ResizeObserver(follow);
    const observe = () => { for (const child of element.children) observer.observe(child); follow(); };
    observe();
    const mutations = new MutationObserver(observe);
    mutations.observe(element, { childList: true });
    return () => {
      mutations.disconnect();
      observer.disconnect();
      element.removeEventListener('scroll', scroll);
      element.removeEventListener('wheel', wheel);
      element.removeEventListener('touchmove', pause);
      element.removeEventListener('keydown', key);
      element.removeEventListener('click', pause);
    };
  }, [open]);
  useLayoutEffect(() => {
    const element = body.current;
    if (element && open && !paused.current) {
      element.scrollTop = element.scrollHeight;
      programmaticTop.current = element.scrollTop;
    }
  }, [open, content]);
  useLayoutEffect(() => {
    if (!open) { clearTimeout(timer.current); paused.current = false; }
    return () => { clearTimeout(timer.current); paused.current = false; };
  }, [open]);
  return body;
}