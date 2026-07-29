/** Built-in Overview analytics tile ids (stable prefs keys). */
export const ANALYTICS_TILE_IDS = [
  "change30",
  "change7",
  "spark7",
  "bestDay",
  "worstDay",
  "topMover",
  "allocation",
  "largest",
  "flow",
] as const;

export type AnalyticsTileId = (typeof ANALYTICS_TILE_IDS)[number];

const KNOWN = new Set<string>(ANALYTICS_TILE_IDS);

export function isAnalyticsTileId(id: string): id is AnalyticsTileId {
  return KNOWN.has(id);
}

/** i18n key under `analytics.*` for a tile label. */
export function analyticsTileLabelKey(id: AnalyticsTileId): string {
  switch (id) {
    case "change30":
      return "analytics.change30";
    case "change7":
      return "analytics.change7";
    case "spark7":
      return "analytics.trajectory7";
    case "bestDay":
      return "analytics.bestDay";
    case "worstDay":
      return "analytics.worstDay";
    case "topMover":
      return "analytics.topMover";
    case "allocation":
      return "analytics.allocation";
    case "largest":
      return "analytics.largest";
    case "flow":
      return "analytics.netFlow30";
  }
}

/**
 * Resolve vault prefs into visible (ordered) + hidden tile lists.
 * Unknown ids are dropped; missing known tiles appear at the end of visible
 * unless they are listed as hidden.
 */
export function resolveAnalyticsLayout(
  order: string[] | null | undefined,
  hidden: string[] | null | undefined,
): { visible: AnalyticsTileId[]; hidden: AnalyticsTileId[] } {
  const hiddenSet = new Set<AnalyticsTileId>();
  for (const id of hidden ?? []) {
    if (isAnalyticsTileId(id)) hiddenSet.add(id);
  }

  const visible: AnalyticsTileId[] = [];
  const seen = new Set<AnalyticsTileId>();
  for (const id of order ?? []) {
    if (!isAnalyticsTileId(id) || hiddenSet.has(id) || seen.has(id)) continue;
    visible.push(id);
    seen.add(id);
  }
  for (const id of ANALYTICS_TILE_IDS) {
    if (hiddenSet.has(id) || seen.has(id)) continue;
    visible.push(id);
    seen.add(id);
  }

  const hiddenList = ANALYTICS_TILE_IDS.filter((id) => hiddenSet.has(id));
  return { visible, hidden: hiddenList };
}
