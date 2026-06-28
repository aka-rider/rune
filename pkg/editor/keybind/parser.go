package keybind

import (
	"errors"
	"fmt"
	"strings"
)

type exprNode interface {
	Eval(ctx ResolverContext) bool
}

type identNode string

func (n identNode) Eval(ctx ResolverContext) bool {
	switch string(n) {
	case "editorFocused":
		return ctx.EditorFocused
	case "hasSelection":
		return ctx.HasSelection
	case "hasMultiCursor":
		return ctx.HasMultiCursor
	case "inCodeFence":
		return ctx.InCodeFence
	case "readOnly":
		return ctx.ReadOnly
	}
	return false
}

type notNode struct {
	inner exprNode
}

func (n notNode) Eval(ctx ResolverContext) bool { return !n.inner.Eval(ctx) }

type andNode struct {
	left, right exprNode
}

func (n andNode) Eval(ctx ResolverContext) bool { return n.left.Eval(ctx) && n.right.Eval(ctx) }

type orNode struct {
	left, right exprNode
}

func (n orNode) Eval(ctx ResolverContext) bool { return n.left.Eval(ctx) || n.right.Eval(ctx) }

type parser struct {
	tokens []string
	pos    int
}

func parseWhen(s string) (exprNode, error) {
	if strings.TrimSpace(s) == "" {
		return nil, nil // always true
	}
	// Tokenize
	var tokens []string
	var current strings.Builder
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch c {
		case ' ', '\t', '\n', '\r':
			if current.Len() > 0 {
				tokens = append(tokens, current.String())
				current.Reset()
			}
		case '(', ')', '!':
			if current.Len() > 0 {
				tokens = append(tokens, current.String())
				current.Reset()
			}
			tokens = append(tokens, string(c))
		case '&', '|':
			if current.Len() > 0 {
				tokens = append(tokens, current.String())
				current.Reset()
			}
			if i+1 < len(s) && s[i+1] == c {
				tokens = append(tokens, string(c)+string(c))
				i++
			} else {
				return nil, fmt.Errorf("unexpected char: %c", c)
			}
		default:
			current.WriteByte(c)
		}
	}
	if current.Len() > 0 {
		tokens = append(tokens, current.String())
	}

	p := &parser{tokens: tokens, pos: 0}
	expr, err := p.parseOr()
	if err != nil {
		return nil, err
	}
	if p.pos < len(tokens) {
		return nil, fmt.Errorf("trailing tokens: %v", tokens[p.pos:])
	}
	return expr, nil
}

func (p *parser) parseOr() (exprNode, error) {
	left, err := p.parseAnd()
	if err != nil {
		return nil, err
	}
	for p.pos < len(p.tokens) && p.tokens[p.pos] == "||" {
		p.pos++
		right, err := p.parseAnd()
		if err != nil {
			return nil, err
		}
		left = orNode{left, right}
	}
	return left, nil
}

func (p *parser) parseAnd() (exprNode, error) {
	left, err := p.parseUnary()
	if err != nil {
		return nil, err
	}
	for p.pos < len(p.tokens) && p.tokens[p.pos] == "&&" {
		p.pos++
		right, err := p.parseUnary()
		if err != nil {
			return nil, err
		}
		left = andNode{left, right}
	}
	return left, nil
}

func (p *parser) parseUnary() (exprNode, error) {
	if p.pos >= len(p.tokens) {
		return nil, errors.New("unexpected EOF")
	}
	if p.tokens[p.pos] == "!" {
		p.pos++
		inner, err := p.parseUnary()
		if err != nil {
			return nil, err
		}
		return notNode{inner}, nil
	}
	return p.parsePrimary()
}

func (p *parser) parsePrimary() (exprNode, error) {
	if p.pos >= len(p.tokens) {
		return nil, errors.New("unexpected EOF in primary")
	}
	tok := p.tokens[p.pos]
	if tok == "(" {
		p.pos++
		expr, err := p.parseOr()
		if err != nil {
			return nil, err
		}
		if p.pos >= len(p.tokens) || p.tokens[p.pos] != ")" {
			return nil, errors.New("missing closing parenthesis")
		}
		p.pos++
		return expr, nil
	}
	p.pos++
	switch tok {
	case "editorFocused", "hasSelection", "hasMultiCursor", "inCodeFence", "readOnly":
		return identNode(tok), nil
	default:
		return nil, fmt.Errorf("unknown identifier: %s", tok)
	}
}
