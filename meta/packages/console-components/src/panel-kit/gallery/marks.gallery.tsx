import { Fragment } from "react";

import { Button, IconButton } from "../button";
import { Chip, StatusDot, StatusPill } from "../chip";
import { KitGlyph } from "../glyphs";
import { StatusMark } from "../status-mark";
import type { Tone } from "../tone";
import { Item, Specimen, SpecimenRow } from "./specimen";
import type { GallerySection } from "./types";

const TONES: Array<[Tone, string]> = [
  ["live", "Working"],
  ["info", "Queued"],
  ["ok", "Healthy"],
  ["warn", "Degraded"],
  ["crit", "Stalled"],
  ["neutral", "Idle"],
  ["locked", "No access"],
];

export const gallerySections: GallerySection[] = [
  {
    id: "marks",
    title: "Status marks and pills",
    order: 20,
    description: "A2.4, A3.3: every status speaks three ways, tone, shape and word. Live is the accent with its ring.",
    render: () => (
      <>
        <SpecimenRow>
          <Specimen title="StatusMark" note="8 px, and 10 px (md)">
            <div className="kg-inline" style={{ gap: "18px 28px" }}>
              {TONES.map(([tone, word]) => (
                <Item key={tone} label={tone}>
                  <span className="kg-inline" style={{ gap: 10 }}>
                    <StatusMark tone={tone} />
                    <StatusMark tone={tone} size="md" />
                    <span style={{ fontSize: 12.5 }}>{word}</span>
                  </span>
                </Item>
              ))}
            </div>
          </Specimen>
        </SpecimenRow>
        <SpecimenRow>
          <Specimen title="StatusPill" note="solid by default, outline for priority, quiet for dense rows where a pill per row is too loud">
            <div className="kg-variants">
              {(["solid", "outline", "quiet"] as const).map((variant) => (
                <Fragment key={variant}>
                  <span className="kg-variants__name">{variant}</span>
                  <div className="kg-inline" style={variant === "quiet" ? { gap: "10px 20px" } : undefined}>
                    {TONES.map(([tone, word]) => (
                      <StatusPill key={tone} tone={tone} variant={variant}>
                        {word}
                      </StatusPill>
                    ))}
                  </div>
                </Fragment>
              ))}
            </div>
          </Specimen>
        </SpecimenRow>
        <SpecimenRow>
          <Specimen title="Dots and chips" note="the older parts speak shape too">
            <div className="kg-stack">
              <div className="kg-inline">
                {TONES.map(([tone]) => (
                  <Item key={tone} label={tone}>
                    <StatusDot tone={tone} live={tone === "live"} />
                  </Item>
                ))}
              </div>
              <div className="kg-inline">
                <Chip tone="crit" outline>
                  High
                </Chip>
                <Chip tone="neutral" outline>
                  Low
                </Chip>
                <Chip tone="ok" dot>
                  Allowed
                </Chip>
                <Chip mono>req_01JBX</Chip>
              </div>
            </div>
          </Specimen>
        </SpecimenRow>
      </>
    ),
  },
  {
    id: "buttons",
    title: "Buttons",
    order: 22,
    description: "A3.2: one primary per view; icon buttons are ghosts named by their label.",
    render: () => (
      <SpecimenRow>
        <Specimen title="Button">
          <div className="kg-stack">
            <div className="kg-inline">
              <Button variant="primary" icon={<KitGlyph name="chat" />}>
                Open chat
              </Button>
              <Button>Details</Button>
              <Button variant="positive">Approve</Button>
              <Button variant="danger">Reject</Button>
              <Button variant="ghost">Reset view</Button>
              <Button disabled>Disabled</Button>
            </div>
            <div className="kg-inline">
              <Button size="sm" variant="primary">
                Claim
              </Button>
              <Button size="sm">Done</Button>
              <Button size="sm" aria-pressed="true">
                Pressed
              </Button>
            </div>
          </div>
        </Specimen>
        <Specimen title="IconButton" note="28 px in headers, 24 px small; quiet, pressed, badge">
          <div className="kg-inline" style={{ gap: "14px 18px" }}>
            <Item label="refresh">
              <IconButton label="Refresh now" glyph="refresh" />
            </Item>
            <Item label="quiet menu">
              <IconButton label="More actions" glyph="ellipsis" quiet />
            </Item>
            <Item label="expanded">
              <IconButton label="More actions" glyph="ellipsis" aria-expanded="true" />
            </Item>
            <Item label="badge">
              <IconButton label="View options" glyph="ellipsis" quiet badge size="sm" />
            </Item>
            <Item label="zoom">
              <span className="kg-inline" style={{ gap: 2 }}>
                <IconButton label="Zoom out" glyph="zoom-out" size="sm" />
                <IconButton label="Zoom in" glyph="zoom-in" size="sm" />
                <IconButton label="Fit" glyph="fit" size="sm" />
              </span>
            </Item>
            <Item label="pane bar">
              <span className="kg-inline" style={{ gap: 2 }}>
                <IconButton label="Split right" glyph="split-right" size="sm" quiet />
                <IconButton label="Split down" glyph="split-down" size="sm" quiet />
                <IconButton label="Close pane" glyph="close" size="sm" quiet />
              </span>
            </Item>
          </div>
        </Specimen>
      </SpecimenRow>
    ),
  },
];
