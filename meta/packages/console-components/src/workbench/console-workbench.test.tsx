import { render, screen } from "@testing-library/react";

import { ConsoleWorkbench } from "./console-workbench";

describe("ConsoleWorkbench", () => {
  test("renders independent sidebar and main regions with optional footer slots", () => {
    render(
      <ConsoleWorkbench
        activityRail={<div>activity rail</div>}
        main={<div>main pane</div>}
        mainFooter={<div>main footer</div>}
        launcher={<div>sidebar pane</div>}
        launcherFooter={<div>sidebar footer</div>}
      />,
    );

    expect(screen.getByText("sidebar pane")).toBeInTheDocument();
    expect(screen.getByText("sidebar footer")).toBeInTheDocument();
    expect(screen.getByText("main pane")).toBeInTheDocument();
    expect(screen.getByText("main footer")).toBeInTheDocument();
    expect(screen.getByText("activity rail")).toBeInTheDocument();
  });
});
