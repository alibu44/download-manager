// Download-list search modelled on Internet Download Manager's "Find file" feature.
// See IDM_SEARCH_ANALYSIS.md for the decompiled evidence behind each rule.

// Same bit layout IDM packs into its saved "dwSearchFlags" value (FUN_004afb10).
export const FLAGS = {
  MATCH_CASE: 0x01,
  WHOLE_STRING: 0x02,
  FILE_NAME: 0x04,
  DESCRIPTION: 0x08,
  LINKS: 0x10, // download URL, parent web page, referer
};

export const DEFAULT_SETTINGS = {
  query: '',
  matchCase: false,
  wholeString: false,
  fileName: true,
  description: false,
  links: false,
};

export function validateSettings(s) {
  if (!s.fileName && !s.description && !s.links) {
    return 'The checkbox for at least one of the search targets must be enabled';
  }
  return null;
}

export function toFlags(s) {
  return (s.matchCase ? FLAGS.MATCH_CASE : 0)
    | (s.wholeString ? FLAGS.WHOLE_STRING : 0)
    | (s.fileName ? FLAGS.FILE_NAME : 0)
    | (s.description ? FLAGS.DESCRIPTION : 0)
    | (s.links ? FLAGS.LINKS : 0);
}

export function fromFlags(flags, query = '') {
  return {
    query,
    matchCase: !!(flags & FLAGS.MATCH_CASE),
    wholeString: !!(flags & FLAGS.WHOLE_STRING),
    fileName: !!(flags & FLAGS.FILE_NAME),
    description: !!(flags & FLAGS.DESCRIPTION),
    links: !!(flags & FLAGS.LINKS),
  };
}

// The query is case-folded once up front; each field is folded per comparison.
function prepare(s) {
  return { ...s, needle: s.matchCase ? s.query : s.query.toUpperCase() };
}

function fold(text, s) {
  return s.matchCase ? text : text.toUpperCase();
}

// File name / description: substring, or exact equality when "whole string" is on.
function matchText(text, s) {
  if (!text) return false;
  const hay = fold(text, s);
  return s.wholeString ? hay === s.needle : hay.includes(s.needle);
}

// Links (FUN_0045ab40): like matchText, but in whole-string mode a URL also
// matches when its host part (between "://" and the next "/") equals the query.
function matchLink(url, s) {
  if (!url) return false;
  const hay = fold(url, s);
  if (!s.wholeString) return hay.includes(s.needle);
  if (hay === s.needle) return true;
  const i = hay.indexOf('://');
  if (i < 0) return false;
  const host = hay.slice(i + 3).split('/')[0];
  return host === s.needle;
}

// FUN_0045fcc0: empty query never matches; targets are OR-ed and short-circuit
// in the order file name -> description -> URL -> parent page -> referer.
export function matchDownload(item, settings) {
  const s = settings.needle === undefined ? prepare(settings) : settings;
  if (!s.query) return false;
  if (s.fileName && matchText(item.fileName, s)) return true;
  if (s.description && matchText(item.description, s)) return true;
  if (s.links) {
    return matchLink(item.url, s) || matchLink(item.parentPage, s) || matchLink(item.referer, s);
  }
  return false;
}

export function findDownloads(items, settings) {
  const s = prepare(settings);
  return items.filter((item) => matchDownload(item, s));
}
