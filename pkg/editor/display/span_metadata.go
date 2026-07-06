package display

// shouldReveal determines if raw markdown should be shown based on cursor.
func shouldReveal(ms mdSpan, lineIdx, cursorLine, cursorCol int) bool {
	switch ms.kind {
	case TokenHeading, TokenBlockquote, TokenHorizontalRule, TokenTaskList, TokenCallout:
		// Line-level reveal
		return lineIdx == cursorLine
	default:
		// Per-token reveal: cursor must be within the token. For nested tokens
		// the emitter stamps the whole outer token's range on every sub-span so
		// they reveal as a unit.
		if lineIdx != cursorLine {
			return false
		}
		start, end := ms.start, ms.end
		if ms.revealSet {
			start, end = ms.revealStart, ms.revealEnd
		}
		return cursorCol >= start && cursorCol < end
	}
}

// spanAltText extracts alt text metadata for image spans.
func spanAltText(ms mdSpan) string {
	if ms.kind == TokenImage {
		return ms.text
	}
	if ms.kind == TokenWikiLink && ms.wikiLinkIsImage {
		return ms.wikiLinkLabel
	}
	return ""
}

// spanImagePath extracts image path metadata.
func spanImagePath(ms mdSpan) string {
	if ms.kind == TokenImage {
		return ms.linkURL
	}
	if ms.kind == TokenWikiLink && ms.wikiLinkIsImage {
		return ms.wikiLinkTarget
	}
	return ""
}

// spanEmbedRef extracts embed reference metadata.
func spanEmbedRef(ms mdSpan) string {
	if ms.kind == TokenImage && ms.delimLeft == 3 {
		return ms.linkURL
	}
	if ms.kind == TokenWikiLink && ms.wikiLinkIsImage {
		return ms.wikiLinkTarget
	}
	return ""
}

// spanCalloutKind extracts callout kind metadata.
func spanCalloutKind(ms mdSpan) string {
	if ms.kind == TokenCallout {
		return ms.linkURL
	}
	return ""
}

// spanWikiLinkTarget extracts wiki link target metadata.
func spanWikiLinkTarget(ms mdSpan) string {
	if ms.kind == TokenWikiLink {
		return ms.wikiLinkTarget
	}
	return ""
}

// spanWikiLinkIsImage extracts wiki link image flag.
func spanWikiLinkIsImage(ms mdSpan) bool {
	if ms.kind == TokenWikiLink {
		return ms.wikiLinkIsImage
	}
	return false
}

// spanLinkURL extracts link URL metadata for TokenLink spans.
func spanLinkURL(ms mdSpan) string {
	if ms.kind == TokenLink {
		return ms.linkURL
	}
	return ""
}
