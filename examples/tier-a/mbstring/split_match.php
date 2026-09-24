<?php
// mb_split (limits, empty matches, the "no support" failure) and
// mb_ereg_match (anchored at the start only).
var_dump(mb_split(',', 'a,b,,c'));
var_dump(mb_split(',', 'a,b,,c', 2), mb_split(',', 'a,b,,c', 0), mb_split(',', 'a,b,,c', -3), mb_split(',', 'a,b,,c', 1));
var_dump(mb_split('', 'abc'), mb_split('x*', 'axbc'), mb_split(',', ''), mb_split(',', 'a,'));
var_dump(mb_split('\s*', 'a b'), mb_split('b', 'abcbd', 3));
var_dump(mb_split('、', '日本、語、テキスト'));
var_dump(mb_split('$', "a\nb"));
var_dump(mb_split("a", "\xff"));
mb_regex_set_options('m');
var_dump(mb_split('^', "a\nb\nc"));
mb_regex_set_options('pr');

echo "-- mb_ereg_match\n";
var_dump(mb_ereg_match('a', 'ba'), mb_ereg_match('a', 'ab'), mb_ereg_match('A', 'ab', 'i'));
var_dump(mb_ereg_match('.*b', "a\nb", ''), mb_ereg_match('.', "\n"), mb_ereg_match('.', "\n", 'm'));
var_dump(mb_ereg_match('\d+', '12ab'), mb_ereg_match('[あ-ん]+', 'ひらがなabc'), mb_ereg_match("a", "\xff"));
