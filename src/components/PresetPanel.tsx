import { useState } from 'react';
import { commands } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';
import { useAsync } from '../hooks/useAsync';
import { presetChoice, presetDraftChanged } from '../pages/browserView';

interface Props {
  /** Controls may be edited: lock state known and free. The Rust core enforces it again. */
  editable: boolean;
  /** First run: the choice has not been made yet, so saving it is the whole point of the panel. */
  firstRun?: boolean;
  /** Called after any attempt so the parent can re-read the lock and history. */
  onAttempted: (message: string) => void;
}

/** Category switches for the recommended block list (docs/PLAN.md feature 2). */
export function PresetPanel({ editable, firstRun = false, onAttempted }: Props) {
  const presets = useAsync(commands.getPresetStatus);
  const [draft, setDraft] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const status = presets.data;

  async function save() {
    if (!status) return;
    try {
      // The first choice sends every category so the saved state is explicit, not implied.
      const choice = firstRun
        ? Object.fromEntries(status.categories.map((c) => [c.id, draft[c.id] ?? c.enabled]))
        : presetChoice(status.categories, draft);
      const outcome = await commands.setPresetCategories(choice);
      setError(null);
      setDraft({});
      presets.reload();
      setNotice(describeOutcome(outcome));
      onAttempted(describeOutcome(outcome));
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <section className="card" aria-labelledby="preset-title">
      <h2 id="preset-title">{firstRun ? '처음 설정: 차단할 카테고리' : '추천 차단 카테고리'}</h2>
      {presets.error && <p className="error">카테고리를 읽지 못했습니다: {presets.error}</p>}
      {error && <p className="error" role="alert">{error}</p>}
      {notice && <p role="status">{notice}</p>}
      {firstRun && (
        <p className="hint">
          공부 시간에 막을 카테고리를 고르세요. 나중에 설정에서 바꿀 수 있습니다. 쇼핑·웹툰은 기본으로 꺼져 있습니다.
          음악 앱(멜론, 스포티파이)은 항상 허용됩니다.
        </p>
      )}
      <ul className="list">
        {(status?.categories ?? []).map((category) => {
          const checked = draft[category.id] ?? category.enabled;
          return (
            <li key={category.id}>
              <label>
                <input
                  type="checkbox"
                  checked={checked}
                  disabled={!editable}
                  onChange={(e) => setDraft({ ...draft, [category.id]: e.target.checked })}
                />{' '}
                <strong>{category.label}</strong>
              </label>
              <div className="hint">
                {category.apps.length > 0 && <>앱 {category.apps.length}개 · </>}
                {category.sites.length > 0 ? category.sites.join(', ') : '사이트 없음'}
              </div>
            </li>
          );
        })}
      </ul>
      <p className="hint">
        앱 식별자는 설치본으로 확인하지 못한 값입니다. 차단되지 않는 앱은 위 &quot;차단 목록&quot;에서 직접 추가하세요.
      </p>
      <button
        type="button"
        disabled={!editable || !status || (!firstRun && !presetDraftChanged(status.categories, draft))}
        onClick={() => void save()}
      >
        {firstRun ? '이 선택으로 시작' : '저장'}
      </button>
    </section>
  );
}
