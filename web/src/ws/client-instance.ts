/** The app-lifetime WS client. Views subscribe through this instance so a
 * tab keeps exactly one socket regardless of mounted components. */
import { WS_URL } from '../api/client';
import { WsClient } from './client';

export const wsClient = new WsClient(WS_URL);
