{#- Fails when the newest `column` value is older than `max_age_seconds`, or
    when there are no rows at all: silence is never fresh. Sources get dbt's
    own `freshness:` block instead; this is for models. The window is passed
    to dateadd in the largest whole unit so a long window (a year in seconds)
    stays inside every adapter's interval range. -#}
{% test freshness(model, column, max_age_seconds) %}
{%- set secs = max_age_seconds | int %}
{%- if secs > 0 and secs % 86400 == 0 %}{% set unit, amount = 'day', secs // 86400 %}
{%- elif secs > 0 and secs % 3600 == 0 %}{% set unit, amount = 'hour', secs // 3600 %}
{%- elif secs > 0 and secs % 60 == 0 %}{% set unit, amount = 'minute', secs // 60 %}
{%- else %}{% set unit, amount = 'second', secs %}
{%- endif %}
select max({{ column }}) as newest
from {{ model }}
having max({{ column }}) is null
    or max({{ column }}) < {{ dbt.dateadd(unit, -1 * amount, dbt.current_timestamp()) }}
{% endtest %}
