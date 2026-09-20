<?php echo "one\n"; ob_flush(); flush(); header('X-Late: 1'); echo "two\n"; var_dump(headers_sent(), ob_get_level(), ob_get_length());
