; Vendored from tree-sitter-postgres 1.2.4's plpgsql/queries/highlights.scm,
; since that crate exports no highlights query of its own. Captures map onto
; rune-syntax's CODE_SCOPES vocabulary by exact name or dotted prefix.
;
; Note: SQL expressions are opaque (captured by external scanner).
; Only PL/pgSQL structural tokens are highlighted here. SQL fragments
; get their highlighting via language injection into the postgres grammar.

(comment) @comment

(integer_literal) @number
(string_literal) @string

(kw_null) @constant.builtin

(identifier) @variable

(block_label (identifier) @label)
(loop_label (identifier) @label)
(end_label) @label

[":=" "=" ".."] @operator

["(" ")"] @punctuation.bracket
["[" "]"] @punctuation.bracket
["<<" ">>"] @punctuation.bracket
"," @punctuation.delimiter
"." @punctuation.delimiter
";" @punctuation.delimiter

[
  (kw_begin)
  (kw_end)
  (kw_declare)
] @keyword

[
  (kw_if)
  (kw_then)
  (kw_elsif)
  (kw_else)
  (kw_case)
  (kw_when)
] @keyword

[
  (kw_loop)
  (kw_while)
  (kw_for)
  (kw_foreach)
  (kw_in)
  (kw_reverse)
  (kw_by)
  (kw_slice)
  (kw_array)
  (kw_exit)
  (kw_continue)
] @keyword

[
  (kw_return)
  (kw_perform)
  (kw_execute)
  (kw_call)
  (kw_do)
  (kw_raise)
  (kw_assert)
  (kw_get)
  (kw_diagnostics)
  (kw_open)
  (kw_fetch)
  (kw_move)
  (kw_close)
  (kw_into)
  (kw_using)
  (kw_strict)
  (kw_next)
  (kw_query)
  (kw_from)
] @keyword

[
  (kw_constant)
  (kw_alias)
  (kw_cursor)
  (kw_scroll)
  (kw_no)
  (kw_is)
  (kw_not)
  (kw_default)
  (kw_collate)
  (kw_type)
  (kw_rowtype)
] @keyword

[
  (kw_exception)
  (kw_or)
  (kw_sqlstate)
] @keyword

[
  (kw_commit)
  (kw_rollback)
  (kw_chain)
  (kw_and)
] @keyword

(raise_level) @keyword

(raise_option
  [
    (kw_message)
    (kw_detail)
    (kw_hint)
    (kw_errcode)
  ] @keyword)

(getdiag_item) @keyword

(fetch_direction) @keyword
