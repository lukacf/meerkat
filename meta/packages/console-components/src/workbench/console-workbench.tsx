import clsx from "clsx";
import type { ReactNode } from "react";

export type ConsoleWorkbenchProps = {
  launcher: ReactNode;
  main: ReactNode;
  activityRail?: ReactNode;
  launcherResizeHandle?: ReactNode;
  launcherHeader?: ReactNode;
  launcherFooter?: ReactNode;
  activityRailResizeHandle?: ReactNode;
  activityRailHeader?: ReactNode;
  activityRailFooter?: ReactNode;
  mainHeader?: ReactNode;
  mainFooter?: ReactNode;
  id?: string;
  className?: string;
};

export function ConsoleWorkbench({
  launcher,
  main,
  activityRail = null,
  launcherResizeHandle = null,
  launcherHeader = null,
  launcherFooter = null,
  activityRailResizeHandle = null,
  activityRailHeader = null,
  activityRailFooter = null,
  mainHeader = null,
  mainFooter = null,
  id,
  className,
}: ConsoleWorkbenchProps) {
  return (
    <div
      className={clsx("cc-theme-scope", "cc-workbench", activityRail && "has-activity-rail", className)}
      data-console-workbench="root"
      id={id}
    >
      <aside className="cc-workbench__launcher" data-console-workbench-part="launcher">
        {launcherHeader ? <div className="cc-workbench__launcher-header" data-console-workbench-part="launcher-header">{launcherHeader}</div> : null}
        <div className="cc-workbench__launcher-body" data-console-workbench-part="launcher-body">{launcher}</div>
        {launcherFooter ? <div className="cc-workbench__launcher-footer" data-console-workbench-part="launcher-footer">{launcherFooter}</div> : null}
      </aside>
      {launcherResizeHandle}
      <section className="cc-workbench__main" data-console-workbench-part="main">
        {mainHeader ? <div className="cc-workbench__main-header" data-console-workbench-part="main-header">{mainHeader}</div> : null}
        <div className="cc-workbench__main-body" data-console-workbench-part="main-body">{main}</div>
        {mainFooter ? <div className="cc-workbench__main-footer" data-console-workbench-part="main-footer">{mainFooter}</div> : null}
      </section>
      {activityRail ? (
        <>
          {activityRailResizeHandle}
          <aside className="cc-workbench__activity" data-console-workbench-part="activity">
            {activityRailHeader ? <div className="cc-workbench__activity-header" data-console-workbench-part="activity-header">{activityRailHeader}</div> : null}
            <div className="cc-workbench__activity-body" data-console-workbench-part="activity-body">{activityRail}</div>
            {activityRailFooter ? <div className="cc-workbench__activity-footer" data-console-workbench-part="activity-footer">{activityRailFooter}</div> : null}
          </aside>
        </>
      ) : null}
    </div>
  );
}
