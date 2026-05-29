package display

import (
	"bytes"

	"github.com/yuin/goldmark"
	"github.com/yuin/goldmark/ast"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/text"
	"github.com/yuin/goldmark/util"
)

var KindWikiLink = ast.NewNodeKind("WikiLink")

type WikiLinkNode struct {
	ast.BaseInline
	Target   []byte
	Fragment []byte
	Label    []byte
	Embed    bool
}

func (n *WikiLinkNode) Kind() ast.NodeKind {
	return KindWikiLink
}

func (n *WikiLinkNode) Dump(src []byte, level int) {
	ast.DumpHelper(n, src, level, map[string]string{
		"Target": string(n.Target),
	}, nil)
}

type wikiLinkParser struct{}

var _open = []byte("[[")
var _embedOpen = []byte("![[")
var _close = []byte("]]")
var _pipe = []byte("|")
var _hash = []byte("#")

func (p *wikiLinkParser) Trigger() []byte {
	return []byte{'!', '['}
}

func (p *wikiLinkParser) Parse(parent ast.Node, block text.Reader, pc parser.Context) ast.Node {
	line, seg := block.PeekLine()
	stop := bytes.Index(line, _close)
	if stop < 0 {
		return nil
	}

	var embed bool
	var innerStart int
	switch {
	case bytes.HasPrefix(line, _open):
		innerStart = 2
	case bytes.HasPrefix(line, _embedOpen):
		embed = true
		innerStart = 3
	default:
		return nil
	}

	innerSeg := text.NewSegment(seg.Start+innerStart, seg.Start+stop)
	target := block.Value(innerSeg)
	label := target

	if idx := bytes.Index(target, _pipe); idx >= 0 {
		target = target[:idx]                                   // [[ ... |
		innerSeg = innerSeg.WithStart(innerSeg.Start + idx + 1) // | ... ]]
		label = block.Value(innerSeg)
	}

	if len(target) == 0 && len(label) == 0 {
		return nil
	}

	fragment := []byte{}
	if idx := bytes.LastIndex(target, _hash); idx >= 0 {
		fragment = target[idx+1:]
		target = target[:idx]
	}

	node := &WikiLinkNode{
		Target:   target,
		Fragment: fragment,
		Label:    label,
		Embed:    embed,
	}

	// Make the label the child text so it's extractable and bounds are computable
	node.AppendChild(node, ast.NewTextSegment(innerSeg))
	block.Advance(stop + 2)
	return node
}

type wikiLinkExtender struct{}

func (e *wikiLinkExtender) Extend(m goldmark.Markdown) {
	m.Parser().AddOptions(
		parser.WithInlineParsers(
			util.Prioritized(&wikiLinkParser{}, 199),
		),
	)
}

var WikiLinkExtension = &wikiLinkExtender{}
