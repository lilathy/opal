import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSortableList } from "../hooks/useSortableList";
import { Switch } from "./Switch";
import {
  ANALYTICS_TILE_IDS,
  analyticsTileLabelKey,
  resolveAnalyticsLayout,
  type AnalyticsTileId,
} from "../lib/analyticsTiles";

type Props = {
  enabled: boolean;
  order: string[];
  hidden: string[];
  busy: boolean;
  onEnabledChange: (enabled: boolean) => void;
  onLayoutChange: (order: AnalyticsTileId[], hidden: AnalyticsTileId[]) => void;
  onReset: () => void;
};

function GripIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden>
      <circle cx="5" cy="4" r="1.25" />
      <circle cx="11" cy="4" r="1.25" />
      <circle cx="5" cy="8" r="1.25" />
      <circle cx="11" cy="8" r="1.25" />
      <circle cx="5" cy="12" r="1.25" />
      <circle cx="11" cy="12" r="1.25" />
    </svg>
  );
}

export function AnalyticsTilesEditor({
  enabled,
  order,
  hidden,
  busy,
  onEnabledChange,
  onLayoutChange,
  onReset,
}: Props) {
  const { t } = useTranslation();
  const listRef = useRef<HTMLDivElement | null>(null);

  const layout = useMemo(
    () => resolveAnalyticsLayout(order, hidden),
    [order, hidden],
  );

  const [visible, setVisible] = useState<AnalyticsTileId[]>(layout.visible);
  const hiddenTiles = layout.hidden;

  useEffect(() => {
    setVisible(resolveAnalyticsLayout(order, hidden).visible);
  }, [order.join("|"), hidden.join("|")]);

  const commit = useCallback(
    (nextVisible: AnalyticsTileId[], nextHidden: AnalyticsTileId[]) => {
      onLayoutChange(nextVisible, nextHidden);
    },
    [onLayoutChange],
  );

  const sortable = useSortableList({
    order: visible,
    onOrderChange: (next) => {
      const ids = next.filter((id): id is AnalyticsTileId =>
        ANALYTICS_TILE_IDS.includes(id as AnalyticsTileId),
      );
      setVisible(ids);
    },
    onDragEnd: () => {
      setVisible((current) => {
        commit(current, hiddenTiles);
        return current;
      });
    },
    containerRef: listRef,
    enabled: enabled && !busy,
  });

  function hideTile(id: AnalyticsTileId) {
    const nextVisible = visible.filter((x) => x !== id);
    const nextHidden = [...hiddenTiles.filter((x) => x !== id), id];
    setVisible(nextVisible);
    commit(nextVisible, nextHidden);
  }

  function showTile(id: AnalyticsTileId) {
    const nextHidden = hiddenTiles.filter((x) => x !== id);
    const nextVisible = [...visible.filter((x) => x !== id), id];
    setVisible(nextVisible);
    commit(nextVisible, nextHidden);
  }

  const canReset =
    hiddenTiles.length > 0 ||
    visible.length !== ANALYTICS_TILE_IDS.length ||
    visible.some((id, i) => id !== ANALYTICS_TILE_IDS[i]);

  return (
    <div className={`analytics-editor${enabled ? "" : " is-disabled"}`}>
      <Switch
        checked={enabled}
        disabled={busy}
        label={t("settings.analyticsEnabled")}
        hint={t("settings.analyticsEnabledHint")}
        onChange={onEnabledChange}
      />

      <div className="analytics-editor__body">
        <div className="analytics-editor__section-head">
          <p className="analytics-editor__section-title">
            {t("settings.analyticsActive")}
          </p>
          <p className="analytics-editor__section-hint">
            {t("settings.analyticsActiveHint")}
          </p>
        </div>

        {visible.length === 0 ? (
          <p className="analytics-editor__empty">{t("settings.analyticsNoneActive")}</p>
        ) : (
          <div
            ref={listRef}
            className={`analytics-editor__list${
              sortable.isDragging ? " is-sorting-active" : ""
            }`}
          >
            {visible.map((id) => {
              const dragging = sortable.draggingId === id;
              return (
                <div
                  key={id}
                  data-sort-id={id}
                  className={`analytics-editor__row${
                    dragging ? " is-sorting-item" : ""
                  }`}
                  onPointerDown={
                    enabled && !busy
                      ? (e) => sortable.beginDrag(e, id)
                      : undefined
                  }
                  onPointerMove={
                    enabled && !busy ? sortable.rowPointerMove : undefined
                  }
                  onPointerUp={
                    enabled && !busy ? sortable.rowPointerEnd : undefined
                  }
                  onPointerCancel={
                    enabled && !busy ? sortable.cancelDrag : undefined
                  }
                  style={enabled && !busy ? { touchAction: "none" } : undefined}
                >
                  <span className="analytics-editor__grip" aria-hidden>
                    <GripIcon />
                  </span>
                  <span className="analytics-editor__label">
                    {t(analyticsTileLabelKey(id))}
                  </span>
                  <button
                    type="button"
                    className="btn btn-sm analytics-editor__action"
                    disabled={busy || !enabled}
                    onClick={(e) => {
                      e.stopPropagation();
                      hideTile(id);
                    }}
                    onPointerDown={(e) => e.stopPropagation()}
                  >
                    {t("settings.analyticsHide")}
                  </button>
                </div>
              );
            })}
          </div>
        )}

        {hiddenTiles.length > 0 ? (
          <>
            <div className="analytics-editor__section-head">
              <p className="analytics-editor__section-title">
                {t("settings.analyticsHidden")}
              </p>
              <p className="analytics-editor__section-hint">
                {t("settings.analyticsHiddenHint")}
              </p>
            </div>
            <div className="analytics-editor__list analytics-editor__list--hidden">
              {hiddenTiles.map((id) => (
                <div key={id} className="analytics-editor__row is-hidden">
                  <span className="analytics-editor__grip analytics-editor__grip--spacer" />
                  <span className="analytics-editor__label">
                    {t(analyticsTileLabelKey(id))}
                  </span>
                  <button
                    type="button"
                    className="btn btn-sm analytics-editor__action"
                    disabled={busy || !enabled}
                    onClick={() => showTile(id)}
                  >
                    {t("settings.analyticsShow")}
                  </button>
                </div>
              ))}
            </div>
          </>
        ) : null}

        {canReset ? (
          <button
            type="button"
            className="btn btn-sm analytics-editor__reset"
            disabled={busy || !enabled}
            onClick={() => {
              setVisible([...ANALYTICS_TILE_IDS]);
              onReset();
            }}
          >
            {t("settings.analyticsReset")}
          </button>
        ) : null}
      </div>
    </div>
  );
}
