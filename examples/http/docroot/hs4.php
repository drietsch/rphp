<?php echo "x"; var_dump(headers_sent()); flush(); var_dump(headers_sent($f, $l), $f, $l); header('X-After-Flush: 1');
