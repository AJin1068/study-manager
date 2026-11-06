import { useState } from 'react';
import { commands, type CharacterView, type ShopItemView } from '../api';
import { useAsync } from '../hooks/useAsync';
import { groupItems, itemAction, itemPriceText, purchaseNotice } from '../pages/characterView';

/** The shop: buy with points, wear, take off. Achievement items are earned, not sold. */
export function ShopPanel(props: { onCharacter: (view: CharacterView) => void }) {
  const shop = useAsync(commands.getShop);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function run(job: () => Promise<string | null>) {
    setBusy(true);
    try {
      setNote(await job());
      shop.reload();
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  }

  function act(item: ShopItemView) {
    const action = itemAction(item);
    if (action === 'buy') {
      void run(async () => purchaseNotice(await commands.purchaseItem(item.id)));
    } else if (action === 'equip') {
      void run(async () => {
        props.onCharacter(await commands.equipItem(item.id));
        return `${item.label}을(를) 입었어요.`;
      });
    } else if (action === 'unequip' && item.slot !== null) {
      const slot = item.slot;
      void run(async () => {
        props.onCharacter(await commands.unequipSlot(slot));
        return `${item.label}을(를) 벗었어요.`;
      });
    }
  }

  const label = { buy: '사기', equip: '입기', unequip: '벗기', locked: '업적 전용', owned: '보유 중' } as const;

  return (
    <section className="card" aria-labelledby="shop-title">
      <h2 id="shop-title">상점</h2>
      {shop.error !== null && <p className="error">상점을 읽지 못했습니다: {shop.error}</p>}
      {shop.data === null ? (
        shop.error === null && <p>불러오는 중</p>
      ) : (
        <>
          <p>보유 포인트 {shop.data.balance}P</p>
          {groupItems(shop.data.items).map((group) => (
            <div key={group.key}>
              <h3>{group.label}</h3>
              <ul className="shop-list">
                {group.items.map((item) => {
                  const action = itemAction(item);
                  const disabled = busy || action === 'locked' || action === 'owned' || (action === 'buy' && !item.affordable);
                  return (
                    <li key={item.id} className="row">
                      <span>{item.label}</span>
                      <span className="hint">{itemPriceText(item)}</span>
                      <button type="button" disabled={disabled} onClick={() => act(item)}>
                        {label[action]}
                      </button>
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
        </>
      )}
      {note !== null && <p role="status" className="message">{note}</p>}
      <p className="hint">그림은 임시 도형입니다. 옷은 모든 캐릭터가 함께 입을 수 있어요.</p>
    </section>
  );
}
