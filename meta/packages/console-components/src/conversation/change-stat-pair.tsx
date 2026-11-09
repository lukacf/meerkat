import clsx from "clsx";

import { formatCount } from "@console-core";

type ChangeStatPairProps = {
  plus: number;
  minus: number;
  className?: string;
};

export function ChangeStatPair({
  plus,
  minus,
  className,
}: ChangeStatPairProps) {
  return (
    <span className={clsx("cc-change-stat", className)}>
      <span className="cc-change-stat__value is-plus">+{formatCount(plus)}</span>
      <span className="cc-change-stat__value is-minus">-{formatCount(minus)}</span>
    </span>
  );
}
