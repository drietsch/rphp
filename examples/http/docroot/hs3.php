<?php echo "x\n"; var_dump(headers_sent()); header('X-After-Big: 1'); trigger_error('a notice', E_USER_NOTICE); $u = $undefined; echo "end\n";
