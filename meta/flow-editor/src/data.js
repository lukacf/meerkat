/* global window */
// Meta Flow Editor boot constants only.
// Live mobpack state, models, tools, skills, and agent definitions hydrate from
// the Meta schema RPC and live in the app/controller state planes.

const GRID = {
  cols: 5,    // initial; grows dynamically based on instances
  rows: 3,    // initial; grows dynamically based on instances
  cellW: 220,
  cellH: 158,
  gapX: 32,
  gapY: 24,
  padX: 56,
  padY: 56,
};
function cellXY(col, row) {
  return {
    x: GRID.padX + col * (GRID.cellW + GRID.gapX),
    y: GRID.padY + row * (GRID.cellH + GRID.gapY),
  };
}

window.META_BOOT = {
  GRID,
  cellXY,
};
