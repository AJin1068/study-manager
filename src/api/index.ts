// The UI talks to the Rust core only through these commands and events (src-tauri/src/commands/).
// Each area lives in its own file; this index re-exports them so callers import from '../api'.

import { browserCommands } from './browser';
import { chatCommands } from './chat';
import { contextCommands } from './context';
import { dashboardCommands } from './dashboard';
import { downtimeCommands } from './downtime';
import { enforceCommands } from './enforce';
import { focusCommands } from './focus';
import { growthCommands } from './growth';
import { llmCommands } from './llm';
import { messageCommands } from './messages';
import { planCommands } from './plan';
import { pointsCommands } from './points';
import { quizCommands } from './quiz';
import { releaseCommands } from './release';
import { remoteCommands } from './remote';
import { retroCommands } from './retro';
import { settingsCommands } from './settings';
import { studyCommands } from './study';
import { timerCommands } from './timer';
import { topicCommands } from './topic';
import { wikiCommands } from './wiki';

export * from './browser';
export * from './chat';
export * from './context';
export * from './dashboard';
export * from './downtime';
export * from './enforce';
export * from './focus';
export * from './growth';
export * from './llm';
export * from './messages';
export * from './plan';
export * from './points';
export * from './quiz';
export * from './release';
export * from './remote';
export * from './retro';
export * from './settings';
export * from './study';
export * from './timer';
export * from './topic';
export * from './wiki';

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
  ...remoteCommands,
  ...llmCommands,
  ...wikiCommands,
  ...quizCommands,
  ...contextCommands,
  ...retroCommands,
  ...messageCommands,
  ...topicCommands,
  ...chatCommands,
  ...growthCommands,
};
