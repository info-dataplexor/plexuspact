{% test format(model, column_name, format) %}
{%- set pattern = plexuspact.format_pattern(format) %}
select {{ column_name }} as failing_value
from {{ model }}
where {{ column_name }} is not null
  and not {{ plexuspact.regex_match(column_name, pattern) }}
{% endtest %}
