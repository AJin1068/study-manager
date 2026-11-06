// The UI talks to the Rust core only through these commands and events (src-tauri/src/commands/).
// Each area lives in its own file; this index re-exports them so callers import from '../api'.

import { browserCommands } from './browser';
import { dashboardCommands } from './dashboard';
import { downtimeCommands } from './downtime';
import { enforceCommands } from './enforce';
import { focusCommands } from './focus';
import { planCommands } from './plan';
import { pointsCommands } from './points';
import { releaseCommands } from './release';
import { settingsCommands } from './settings';
import { studyCommands } from './study';
import { timerCommands } from './timer';

export * from './browser';
export * from './dashboard';
export * from './downtime';
export * from './enforce';
export * from './focus';
export * from './plan';
export * from './points';
export * from './release';
export * from './settings';
export * from './study';
export * from './timer';

export const commands = {
  ...settingsCommands,
  ...studyCommands,
  ...downtimeCommands,
  ...planCommands,
  ...timerCommands,
  ...enforceCommands,
  ...releaseCommands,
  ...browserCommands,
  ...focusCommands,
  ...pointsCommands,
  ...dashboardCommands,
};
