import type { ChromeLike } from './chromeApi';
import { LinkController } from './controller';

declare const chrome: ChromeLike;

new LinkController(chrome).register();
