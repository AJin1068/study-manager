import { ChatPanel } from '../components/ChatPanel';

/** The small window opened from the tray ("챗봇 입력"); the main window's tabs do not apply here. */
export function QuickChatScreen() {
  return (
    <main className="page">
      <ChatPanel compact />
    </main>
  );
}
