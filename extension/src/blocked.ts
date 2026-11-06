import { parseBlockedParams, reasonText } from './verdict';

const info = parseBlockedParams(window.location.search);
const domain = document.getElementById('domain');
const reason = document.getElementById('reason');
if (domain) domain.textContent = info.domain;
if (reason) reason.textContent = reasonText(info.reasonCode);
