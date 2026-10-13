import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// vitest 非 globals 模式下 RTL 不会自动清理 DOM，跨用例会残留
afterEach(() => cleanup());

// jsdom 缺失的浏览器 API（antd 组件依赖）

Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  }),
});

if (!window.ResizeObserver) {
  class ResizeObserverMock {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  window.ResizeObserver = ResizeObserverMock as unknown as typeof ResizeObserver;
}

// rc-util 通过 getComputedStyle(elt, pseudoElt) 测滚动条，jsdom 不支持伪元素重载
const origGetComputedStyle = window.getComputedStyle;
window.getComputedStyle = (elt: Element, pseudoElt?: string | null) =>
  origGetComputedStyle(elt, pseudoElt as string | undefined) as CSSStyleDeclaration;
