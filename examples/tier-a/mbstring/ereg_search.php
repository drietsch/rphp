<?php
// The mb_ereg_search_* cursor: errors before init, a pattern that
// outlives a failed search, positions, setpos with negative offsets,
// empty matches that do not advance, groups and names, invalid strings,
// options that throw after the search ran.
function show($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}
show(fn() => mb_ereg_search_getregs());
show(fn() => mb_ereg_search_getpos());
show(fn() => mb_ereg_search());
show(fn() => mb_ereg_search('a'));
show(fn() => mb_ereg_search_setpos(5));
show(fn() => mb_ereg_search_setpos(-5));
show(fn() => mb_ereg_search_getpos());
show(fn() => mb_ereg_search_init("abcabc"));
show(fn() => mb_ereg_search());
show(fn() => [mb_ereg_search('b'), mb_ereg_search_getpos(), mb_ereg_search_getregs()]);
show(fn() => [mb_ereg_search_pos(), mb_ereg_search_getpos()]);
show(fn() => [mb_ereg_search_pos(), mb_ereg_search_getpos(), mb_ereg_search_getregs()]);
show(fn() => [mb_ereg_search_setpos(-2), mb_ereg_search_getpos()]);
show(fn() => mb_ereg_search_setpos(7));
show(fn() => mb_ereg_search_setpos(-7));
show(fn() => [mb_ereg_search_regs('(?<x>c)|(?<y>q)'), mb_ereg_search_getpos()]);
show(fn() => mb_ereg_search_init("abc", "(", "i"));
show(fn() => [mb_ereg_search_init("\xff", "a"), mb_ereg_search_getpos(), mb_ereg_search()]);
show(fn() => mb_ereg_search_init("xyz", "y", "i"));
show(fn() => mb_ereg_search_regs("z", "q"));
show(fn() => [mb_ereg_search_getpos(), mb_ereg_search_regs(), mb_ereg_search_getregs()]);
show(fn() => [mb_ereg_search_init("aaa", "x*"), mb_ereg_search_pos(), mb_ereg_search_pos(), mb_ereg_search_getpos()]);
show(fn() => [mb_ereg_search_setpos(1), mb_ereg_search_regs("(a)(b)?"), mb_ereg_search_getregs()]);

echo "-- multibyte walk\n";
mb_ereg_search_init("日本語 テキスト 日本", "日本");
while ($p = mb_ereg_search_pos()) {
    echo implode(",", $p), " ", mb_ereg_search_getpos(), "\n";
}
mb_ereg_search_init("a1b22c333", "(?<d>\d+)");
while ($r = mb_ereg_search_regs()) {
    echo json_encode($r), "\n";
}
