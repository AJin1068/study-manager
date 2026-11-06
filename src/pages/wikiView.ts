import type { AskOutcome, IngestView } from '../api';
import { llmErrorText } from './llmView';

const SKIP_REASONS: Record<string, string> = {
  not_text: '텍스트가 아니라 건너뜀',
  too_large: '너무 커서 건너뜀',
  unreadable: '읽지 못해 건너뜀',
  not_a_file: '파일이 아니라 건너뜀',
  unsupported_type: '지원하지 않는 형식',
  too_many_files: '파일이 너무 많아 일부만 가져옴',
  too_many_steps: '분량이 많아 건너뜀',
  pdf_unsupported: 'PDF 읽기는 아직 지원하지 않습니다. 텍스트로 붙여넣어 주세요.',
  pdf_no_text: '글자를 찾지 못한 PDF(스캔본)라 건너뜀',
  pdf_parser_panicked: 'PDF를 읽다가 문제가 생겨 건너뜀',
  pdf_failed: 'PDF를 읽지 못함',
};

export function skipReasonText(reason: string): string {
  return SKIP_REASONS[reason] ?? reason;
}

export function ingestSummary(view: IngestView): string {
  switch (view.status) {
    case 'ready':
      return view.usedFallback
        ? 'LLM 없이 제목 단위로 나눈 초안입니다. 확인 후 저장하세요.'
        : `${view.proposals.length}개 페이지 초안입니다. 확인 후 저장하세요.`;
    case 'partial':
      return `${view.processedSteps}/${view.totalSteps} 단계까지 처리했습니다. 이어서 처리할 수 있습니다.`;
    case 'stopped':
      return `${view.processedSteps}/${view.totalSteps} 단계에서 멈췄습니다: ${llmErrorText(view.stoppedBy ?? 'unknown')} 지금까지의 초안은 저장할 수 있고, 나머지는 이어서 처리할 수 있습니다.`;
    case 'empty':
      return `쓸 만한 초안이 나오지 않았습니다${view.rejectedSteps > 0 ? ` (형식이 맞지 않은 응답 ${view.rejectedSteps}건 버림)` : ''}.`;
    case 'unsupported':
      return '가져올 수 있는 내용이 없습니다.';
  }
}

export function canResume(view: IngestView): boolean {
  return view.previewId !== null && (view.status === 'partial' || view.status === 'stopped');
}

export function askSummary(outcome: AskOutcome): string {
  switch (outcome.status) {
    case 'notInWiki':
      return outcome.hint;
    case 'answer':
      return outcome.droppedCitations > 0 ? `${outcome.droppedCitations}개의 근거 없는 인용은 뺐습니다.` : '';
    case 'ungrounded':
      return '모델의 답이 위키 페이지를 근거로 하지 않아 버렸습니다. 찾은 페이지를 보여 드립니다.';
    case 'unavailable':
      return `${llmErrorText(outcome.code)} 대신 찾은 페이지를 보여 드립니다.`;
  }
}
