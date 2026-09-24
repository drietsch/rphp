<?php
function tags(string $html) {
    file_put_contents('meta.html', $html);
    var_dump(get_meta_tags('meta.html'));
}
tags(<<<'HTML'
<html><head>
<meta name="author" content="name">
<meta name="keywords" content="php documentation">
<meta name="DESCRIPTION" content="a php manual">
<meta name="geo.position" content="49.33;-86.59">
<meta name='single' content='quoted'>
<meta name=bare content=word>
<meta name="no content">
<meta content="no name">
<meta name = "spaced" content = "x">
<meta
  name="multi"
  content="line">
<META NAME="Upper" CONTENT="Case">
<meta name="dup" content="first">
<meta name="dup" content="second">
<meta name="123" content="numeric">
<meta name="a.b+c*d?e[f^g]h$i(j)k l" content="unsafe">
<meta http-equiv="refresh" content="5">
<meta content="before" name="after">
<link name="notmeta" content="x">
<meta name="it's" content="apos">
</head>
<meta name="after_head" content="ignored">
HTML);
tags("<meta name=\"a\" content=\"b\"/>\n<meta name=\"c\" content=\"d\" />");
tags("<meta name=\"x\" content=\"unterminated");
tags("<meta name=\"early\" content=\"yes\"></head><meta name=\"late\" content=\"no\">");
tags("no tags at all");
tags("");
tags("<meta name=\"k\" content=\"v\"><meta name=\"k2\"\x00 content=\"v2\">");
tags("<meta name=\"text\" content=text/html; charset=utf-8>");
unlink('meta.html');
var_dump(get_meta_tags('nope.html'));
try { get_meta_tags(''); } catch (\ValueError $e) { echo $e->getMessage(), "\n"; }
try { get_meta_tags("a\0b"); } catch (\ValueError $e) { echo $e->getMessage(), "\n"; }
