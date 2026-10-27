import * as React from "react";

import {
  createMetaConsoleController,
  type MetaConsoleController,
  type MetaConsoleTransport,
} from "./headless";

export function useMetaConsoleController({
  transport,
}: {
  transport: MetaConsoleTransport;
}): MetaConsoleController {
  return React.useMemo(
    () => createMetaConsoleController({ transport }),
    [transport],
  );
}
