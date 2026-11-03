# Changelog

All notable changes to Meerkat Connect are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Fixed

- A setup check that answers for an already retained validation id with
  something that is not a valid proposal (for example, a file list naming both
  a declaration and an activation document) now invalidates the retained
  proposal, as a changed answer already did, so a selection made from that
  proposal is no longer current. Before, the check was refused but the earlier
  selection stayed current.
