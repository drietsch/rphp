<?php
#[Attr]
#[Attr(1, 2)]
#[Attr, Other]
#[
    Multi(
        line: true,
    ),
]
#[Attr] function f(#[Param] $x) {}
$s = "#[not]"; $t = '#[not]'; // #[not]
#[Attr]#[Attr2]
#[]
# [ not
#	[ not
#[Attr] #not attr
$a->#[x]
b;
"$a[#]"; `#[x]`; <<<EOT
#[x]
EOT;
#[
