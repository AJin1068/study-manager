//! The phrase pools. Code constants, so they are reviewed like any other text and tested for
//! being non-empty and free of forbidden words. General phrases name no subject or page; wiki
//! phrases carry a `{concept}` slot for a page's concept name.

use super::{Kind, Tone};

type Pools = [&'static [&'static str]; 5];

/// Index order of every pool: gentle, warm, normal, firm, harsh.
const DISTRACTION: Pools = [
    &[
        "잠깐 쉬고 싶었군요. 괜찮아요, 천천히 돌아와요.",
        "한눈을 팔았어도 괜찮아요. 지금부터 다시 해봐요.",
    ],
    &[
        "앗, 다른 데로 가려던 거죠? 조금만 더 집중해봐요.",
        "지금은 공부 시간이에요. 같이 다시 시작해요.",
    ],
    &[
        "공부 시간입니다. 다시 돌아가세요.",
        "차단된 곳에 가려고 했어요. 하던 공부로 돌아가요.",
    ],
    &[
        "지금 거기 갈 때가 아니에요. 바로 돌아가세요.",
        "계획을 정한 건 본인입니다. 지키세요.",
    ],
    &[
        "또 딴짓입니까. 약속한 시간은 아직 안 끝났습니다.",
        "그 손가락 거두고 책으로 돌아가세요. 지금 당장.",
    ],
];
const STREAK: Pools = [
    &[
        "며칠째 꾸준히 해냈어요. 정말 잘하고 있어요.",
        "이어온 날들이 쌓였어요. 오늘도 멋져요.",
    ],
    &[
        "연속 달성 중이에요! 이 흐름 너무 좋아요.",
        "하루하루 쌓인 게 보여요. 대단해요.",
    ],
    &[
        "연속 달성이 이어지고 있습니다. 이 페이스를 유지하세요.",
        "꾸준함이 기록으로 남고 있어요.",
    ],
    &[
        "연속 기록이 이어지는 중입니다. 오늘 끊지 마세요.",
        "여기서 멈추면 기록이 아깝죠. 계속 가세요.",
    ],
    &[
        "연속 기록이 아깝지 않다면 계속 하세요. 끊기는 건 순식간입니다.",
        "기록은 지키는 사람의 것입니다. 오늘도 증명하세요.",
    ],
];
const SET_END: Pools = [
    &[
        "한 세트를 마쳤어요. 수고 많았어요.",
        "한 세트 끝! 잘 해냈어요.",
    ],
    &[
        "한 세트 끝났어요! 다음 세트도 같이 가요.",
        "잘했어요. 조금만 더 힘내요.",
    ],
    &[
        "한 세트 완료. 다음 세트를 이어가세요.",
        "한 세트를 마쳤습니다. 흐름을 유지하세요.",
    ],
    &[
        "한 세트 끝. 방심하지 말고 다음 세트에 들어가세요.",
        "이제 절반입니다. 멈추지 마세요.",
    ],
    &[
        "겨우 한 세트입니다. 만족하기엔 이릅니다.",
        "한 세트로 뭘 배웠는지 스스로 물어보세요.",
    ],
];
const RANDOM: Pools = [
    &[
        "지금 잘 하고 있어요. 숨 한 번 쉬고 이어가요.",
        "차근차근 하면 돼요. 응원할게요.",
    ],
    &[
        "잘 하고 있어요! 자세도 한 번 펴봐요.",
        "조금만 더요. 지금 흐름 좋아요.",
    ],
    &[
        "집중 상태를 유지하세요. 남은 시간 충실히.",
        "지금 하는 것에만 집중하세요.",
    ],
    &[
        "폰은 멀리 있나요? 지금 하던 것에 몰두하세요.",
        "남은 시간도 아껴 쓰세요.",
    ],
    &[
        "집중이 흐트러진 건 아닙니까. 정신 차리세요.",
        "시간은 가고 있습니다. 손을 움직이세요.",
    ],
];

const WIKI: Pools = [
    &[
        "어제 정리한 {concept}, 천천히 떠올려볼까요?",
        "{concept}을(를) 다시 한 번 훑어보면 좋을 것 같아요.",
    ],
    &[
        "{concept}, 3단계로 말해볼 수 있을까요?",
        "잠깐, {concept}에 대해 기억나는 걸 떠올려봐요.",
    ],
    &[
        "{concept}, 3단계로 설명할 수 있나요?",
        "{concept}의 핵심을 한 문장으로 말해보세요.",
    ],
    &[
        "{concept}, 아직 설명 못 하면 다시 보세요.",
        "{concept}을(를) 보지 않고 설명해보세요.",
    ],
    &[
        "{concept}도 제대로 설명 못 하면서 쉬고 싶습니까.",
        "{concept}, 지금 바로 설명해보세요. 못 하면 다시 공부하세요.",
    ],
];

fn tone_index(tone: Tone) -> usize {
    match tone {
        Tone::Gentle => 0,
        Tone::Warm => 1,
        Tone::Normal => 2,
        Tone::Firm => 3,
        Tone::Harsh => 4,
    }
}

fn pools(kind: Kind) -> &'static Pools {
    match kind {
        Kind::Distraction => &DISTRACTION,
        Kind::Streak => &STREAK,
        Kind::SetEnd => &SET_END,
        Kind::Random => &RANDOM,
    }
}

fn pick(pool: &'static [&'static str], seed: u64) -> &'static str {
    // Pools are non-empty constants; the guard keeps the lookup panic-free regardless.
    let len = pool.len().max(1);
    pool.get(usize::try_from(seed % len as u64).unwrap_or(0))
        .copied()
        .unwrap_or("")
}

/// A general phrase: no subject, no page, safe for a lock screen.
pub fn general(kind: Kind, tone: Tone, seed: u64) -> &'static str {
    pick(pools(kind)[tone_index(tone)], seed)
}

/// A phrase that asks about `concept`.
pub fn wiki(tone: Tone, concept: &str, seed: u64) -> String {
    pick(WIKI[tone_index(tone)], seed).replace("{concept}", concept)
}

#[cfg(test)]
pub(super) fn all_phrases() -> impl Iterator<Item = (&'static str, Tone)> {
    let kinds = [&DISTRACTION, &STREAK, &SET_END, &RANDOM, &WIKI];
    kinds.into_iter().flat_map(|pools| {
        Tone::ALL
            .into_iter()
            .flat_map(move |t| pools[tone_index(t)].iter().map(move |p| (*p, t)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORBIDDEN: &[&str] = &[
        "씨발",
        "병신",
        "죽어",
        "꺼져",
        "멍청",
        "바보",
        "쓰레기",
        "한심",
    ];

    #[test]
    fn every_kind_and_tone_has_non_empty_phrases_without_forbidden_words() {
        for kind in Kind::ALL {
            for tone in Tone::ALL {
                assert!(
                    pools(kind)[tone_index(tone)].len() >= 2,
                    "{kind:?}/{tone:?} needs at least two phrases"
                );
            }
        }
        for tone in Tone::ALL {
            assert!(WIKI[tone_index(tone)].len() >= 2);
        }
        for (phrase, tone) in all_phrases() {
            assert!(!phrase.trim().is_empty(), "{tone:?}");
            for word in FORBIDDEN {
                assert!(!phrase.contains(word), "{phrase:?} contains {word:?}");
            }
        }
    }

    #[test]
    fn general_phrases_have_no_slot_and_wiki_phrases_have_exactly_one() {
        for kind in Kind::ALL {
            for tone in Tone::ALL {
                for phrase in pools(kind)[tone_index(tone)] {
                    assert!(!phrase.contains('{'), "{phrase}");
                }
            }
        }
        for tone in Tone::ALL {
            for phrase in WIKI[tone_index(tone)] {
                assert_eq!(phrase.matches("{concept}").count(), 1, "{phrase}");
            }
        }
    }

    #[test]
    fn the_seed_selects_a_phrase_and_the_wiki_slot_is_filled() {
        assert_ne!(
            general(Kind::Random, Tone::Normal, 0),
            general(Kind::Random, Tone::Normal, 1)
        );
        let text = wiki(Tone::Normal, "미분", 0);
        assert!(text.contains("미분") && !text.contains('{'));
    }

    #[test]
    fn tones_differ_from_each_other() {
        let lines: Vec<&str> = Tone::ALL
            .iter()
            .map(|t| general(Kind::Distraction, *t, 0))
            .collect();
        for (i, a) in lines.iter().enumerate() {
            assert!(lines.iter().skip(i + 1).all(|b| a != b));
        }
    }
}
