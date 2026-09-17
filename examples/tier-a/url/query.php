<?php
// Tier-A differential: http_build_query (nested arrays, numeric prefixes,
// custom separators, PHP_QUERY_RFC1738 vs RFC3986 = 1 / 2, null/bool/float
// values, objects' public properties) and parse_str (`[]`/`[k]` nesting,
// dots and spaces in names, scalar/array replacement, decoding of `+` and
// `%xx`, the max_input_vars limit).

echo http_build_query(["a" => 1, "b" => "x y", "c" => [1, 2, "k" => ["z" => true]], "d" => null, "e" => false, "f" => 1.5, "g" => ""]), "\n";
echo http_build_query([1, 2, "x" => 3], "p_"), "\n";
echo http_build_query([1, "k" => [2]], "a b"), "\n";
echo http_build_query(["a" => 1, "b" => 2], "", ";"), "\n";
echo http_build_query(["a" => 1, "b" => 2], "", ""), "\n";
echo http_build_query(["a" => 1, "b" => 2], "", "&amp;"), "\n";
echo http_build_query(["a b" => "c d~"], "", "&", 2), "\n";
echo http_build_query(["a b" => "c d~"], "", "&", 1), "\n";
echo http_build_query([]), "|\n";
echo http_build_query(["a" => [[]]]), "|\n";
echo http_build_query(["a" => ["b" => ["c" => 1]]]), "\n";
echo http_build_query(["arr" => [1, 2], "k[]" => "v"]), "\n";
echo http_build_query([5 => "a", "b" => "c"]), "\n";
echo http_build_query(["x" => 1e25, "y" => 0.1, "z" => -0.0, "w" => 100.0]), "\n";

class Pt {
    public $a = 1;
    protected $hidden = 2;
    public $b = ["c" => 2];
}
$o = new Pt;
echo http_build_query($o), "\n";
echo http_build_query(["o" => $o]), "\n";
echo http_build_query(["a" => 1], "", "&", 5), "\n";

// parse_str fills the by-reference result array.
parse_str("a=1&b[]=2&b[]=3&c[k]=v&d.e=f&g h=i&j[k][l]=m&n&o=&p[=q&r]=s&t[a]b=u&arr[]=1&arr[x]=2&arr[]=3&&=z&x[1]=a&x[]=b", $out);
var_dump($out);
parse_str("a%20b=c%2Bd&e=f+g&h[]", $o2);
var_dump($o2);
parse_str("", $o3);
var_dump($o3);
parse_str("a[]=1&a=2&b=1&b[]=2", $o4);
var_dump($o4);
parse_str("a[0]=x&a[]=y&a[-1]=z&a[]=w", $o5);
var_dump($o5);
parse_str("_[a]=1&.x=2&a..b=3&a.b[c]=4& lead=5&x[a b]=6&x[a.b]=7&y[ ]=8&z[ k]=9", $o6);
var_dump($o6);
parse_str("a[[]=1&a[]]=2&a[x[y]=3&b[c][d=4", $o7);
var_dump($o7);
parse_str("k=v;l=w", $o8);
var_dump($o8);
