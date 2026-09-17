<?php
// line comment
# hash comment
/* block */
/** doc */
/**doc-ish*/
/***/
/**/
/**
 * multi
 * line
 */
/*
 multi line plain
*/
$a = 1; // trailing not closed here
$b = 2; # trailing
$c = 3; /* inline */ $d = 4;
#[Attr]
# [ not attribute
#[Attr(1, "x")]
/** @var int $x */ $x = 1;
$y = 5;/**/$z = 6;/***/$w = 7;/** */$v = 8;/**	*/
//
#
/*/ still open */ $u = 9;
/**
