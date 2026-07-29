import {
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
} from "react";

const FLIP_MS = 280;
const FLIP_EASE = "cubic-bezier(0.22, 1, 0.36, 1)";

type DragState = {
  id: string;
  pointerId: number;
  startY: number;
  originTop: number;
};

type Options = {
  order: string[];
  onOrderChange: (next: string[]) => void;
  onDragEnd?: () => void;
  containerRef: RefObject<HTMLDivElement | null>;
  enabled: boolean;
  itemSelector?: string;
};

function itemEl(
  container: HTMLElement,
  itemSelector: string,
  id: string,
): HTMLElement | null {
  return container.querySelector<HTMLElement>(`${itemSelector}[data-sort-id="${id}"]`);
}

export function useSortableList({
  order,
  onOrderChange,
  onDragEnd,
  containerRef,
  enabled,
  itemSelector = "[data-sort-id]",
}: Options) {
  const [draggingId, setDraggingId] = useState<string | null>(null);

  const dragState = useRef<DragState | null>(null);
  const flipSnapshot = useRef<Map<string, DOMRect>>(new Map());
  const flipTimers = useRef<Map<string, number>>(new Map());
  const orderRef = useRef(order);
  const onOrderChangeRef = useRef(onOrderChange);
  const onDragEndRef = useRef(onDragEnd);

  orderRef.current = order;
  onOrderChangeRef.current = onOrderChange;
  onDragEndRef.current = onDragEnd;

  const snapshotRects = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;
    const map = new Map<string, DOMRect>();
    container.querySelectorAll<HTMLElement>(itemSelector).forEach((el) => {
      const id = el.dataset.sortId;
      if (id) map.set(id, el.getBoundingClientRect());
    });
    flipSnapshot.current = map;
  }, [containerRef, itemSelector]);

  const clearFlipStyles = useCallback((el: HTMLElement) => {
    const id = el.dataset.sortId ?? "";
    const tid = flipTimers.current.get(id);
    if (tid != null) window.clearTimeout(tid);
    el.style.transition = "";
    el.style.transform = "";
    flipTimers.current.delete(id);
  }, []);

  useLayoutEffect(() => {
    if (!enabled) return;
    const container = containerRef.current;
    if (!container) return;

    container.querySelectorAll<HTMLElement>(itemSelector).forEach((el) => {
      const id = el.dataset.sortId;
      if (!id || id === dragState.current?.id) return;

      const prev = flipSnapshot.current.get(id);
      if (!prev) return;

      const next = el.getBoundingClientRect();
      const dy = prev.top - next.top;
      if (Math.abs(dy) < 0.5) return;

      clearFlipStyles(el);
      el.style.transition = "none";
      el.style.transform = `translate3d(0, ${dy}px, 0)`;

      requestAnimationFrame(() => {
        el.style.transition = `transform ${FLIP_MS}ms ${FLIP_EASE}`;
        el.style.transform = "translate3d(0, 0, 0)";
        const tid = window.setTimeout(() => clearFlipStyles(el), FLIP_MS + 48);
        flipTimers.current.set(id, tid);
      });
    });
  }, [order, enabled, containerRef, itemSelector, clearFlipStyles]);

  const finishDrag = useCallback(() => {
    const drag = dragState.current;
    const container = containerRef.current;

    if (drag && container) {
      const el = itemEl(container, itemSelector, drag.id);
      if (el) {
        if (el.hasPointerCapture(drag.pointerId)) {
          el.releasePointerCapture(drag.pointerId);
        }
        el.style.transition = `transform ${FLIP_MS}ms ${FLIP_EASE}`;
        el.style.transform = "translate3d(0, 0, 0) scale(1)";
        el.style.zIndex = "";
        window.setTimeout(() => {
          el.style.transition = "";
          el.style.transform = "";
        }, FLIP_MS);
      }
    }

    dragState.current = null;
    setDraggingId(null);
    onDragEndRef.current?.();
  }, [containerRef, itemSelector]);

  /** Attach to each sortable row — must live on the same element that captures the pointer. */
  const rowPointerMove = useCallback(
    (e: ReactPointerEvent<HTMLElement>) => {
      const drag = dragState.current;
      const container = containerRef.current;
      if (!drag || e.pointerId !== drag.pointerId || !container) return;

      const el = itemEl(container, itemSelector, drag.id);
      if (!el) return;

      e.preventDefault();

      const desiredTop = drag.originTop + (e.clientY - drag.startY);
      el.style.transition = "none";
      el.style.transform = "";
      const naturalTop = el.getBoundingClientRect().top;
      const dy = desiredTop - naturalTop;
      el.style.transform = `translate3d(0, ${dy}px, 0) scale(1.02)`;
      el.style.zIndex = "10";

      const currentOrder = orderRef.current;
      const rows = [...container.querySelectorAll<HTMLElement>(itemSelector)];
      const draggedIndex = rows.findIndex((r) => r.dataset.sortId === drag.id);
      const pointerY = e.clientY;

      for (let i = 0; i < rows.length; i++) {
        if (i === draggedIndex) continue;
        const rect = rows[i].getBoundingClientRect();
        const center = rect.top + rect.height / 2;
        const shouldSwap =
          (i < draggedIndex && pointerY < center) || (i > draggedIndex && pointerY > center);
        if (shouldSwap) {
          const next = [...currentOrder];
          const [moved] = next.splice(draggedIndex, 1);
          next.splice(i, 0, moved);
          snapshotRects();
          onOrderChangeRef.current(next);
          break;
        }
      }
    },
    [containerRef, itemSelector, snapshotRects],
  );

  const rowPointerEnd = useCallback(
    (e: ReactPointerEvent<HTMLElement>) => {
      const drag = dragState.current;
      if (!drag || e.pointerId !== drag.pointerId) return;
      finishDrag();
    },
    [finishDrag],
  );

  const beginDrag = useCallback(
    (e: ReactPointerEvent<HTMLElement>, id: string) => {
      if (!enabled) return;
      const container = containerRef.current;
      const el = container ? itemEl(container, itemSelector, id) : null;
      if (!el || !container) return;

      e.preventDefault();
      e.stopPropagation();

      snapshotRects();
      dragState.current = {
        id,
        pointerId: e.pointerId,
        startY: e.clientY,
        originTop: el.getBoundingClientRect().top,
      };
      setDraggingId(id);

      el.setPointerCapture(e.pointerId);
    },
    [enabled, containerRef, itemSelector, snapshotRects],
  );

  const cancelDrag = useCallback(() => {
    const drag = dragState.current;
    const container = containerRef.current;
    if (drag && container) {
      const el = itemEl(container, itemSelector, drag.id);
      if (el) {
        if (el.hasPointerCapture(drag.pointerId)) {
          el.releasePointerCapture(drag.pointerId);
        }
        el.style.transition = "";
        el.style.transform = "";
        el.style.zIndex = "";
      }
    }
    dragState.current = null;
    setDraggingId(null);
  }, [containerRef, itemSelector]);

  return {
    draggingId,
    isDragging: draggingId != null,
    beginDrag,
    rowPointerMove,
    rowPointerEnd,
    cancelDrag,
  };
}
