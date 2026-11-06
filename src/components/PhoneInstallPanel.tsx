import { useEffect, useState } from 'react';
import { commands, type PhoneInstallView } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';
import { svgDataUri } from '../pages/notifyView';
import { ANDROID_INSTALL_STEPS, IOS_ALTERNATIVES, SIDELOAD_NOTE, iosStatusText } from '../pages/phoneInstallView';

/** Settings card: QR of the APK link, install steps, and the iPhone "not supported" explanation. */
export function PhoneInstallPanel() {
  const [view, setView] = useState<PhoneInstallView | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    commands
      .getPhoneInstall()
      .then(setView)
      .catch((err: unknown) => setNote(`실패: ${String(err)}`));
  }, []);

  async function save() {
    if (draft === null) return;
    try {
      setNote(describeOutcome(await commands.updatePhoneApkLink(draft)));
      setView(await commands.getPhoneInstall());
      setDraft(null);
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  return (
    <section className="card" aria-labelledby="phone-install-title">
      <h2 id="phone-install-title">폰 앱 설치</h2>
      {view && (
        <>
          <h3>안드로이드</h3>
          <ol className="list">
            {ANDROID_INSTALL_STEPS.map((step) => (
              <li key={step}>{step}</li>
            ))}
          </ol>
          {view.qrSvg ? (
            <img src={svgDataUri(view.qrSvg)} alt="폰 앱 설치 QR 코드" width={200} height={200} />
          ) : (
            <p className="error">QR을 만들지 못했습니다. 링크를 확인하세요.</p>
          )}
          <p className="hint"><code>{view.link}</code></p>
          <p className="warn">{SIDELOAD_NOTE}</p>
          <div className="row">
            <label>
              설치 링크{' '}
              <input
                type="text"
                size={50}
                value={draft ?? view.link}
                onChange={(e) => setDraft(e.target.value)}
              />
            </label>
            <button type="button" disabled={draft === null} onClick={() => void save()}>저장</button>
            <button type="button" onClick={() => setDraft(view.defaultLink)}>기본값</button>
          </div>
          <h3>iPhone</h3>
          <p className="hint">{iosStatusText(view.ios)}</p>
          <ul className="list">
            {IOS_ALTERNATIVES.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </>
      )}
      {note && <p role="status" className="message">{note}</p>}
    </section>
  );
}
