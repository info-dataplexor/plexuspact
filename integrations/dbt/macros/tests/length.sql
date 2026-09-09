{% test length(model, column_name, exact=none, min=none, max=none) %}
{%- set conditions = [] %}
{%- if exact is not none %}{% do conditions.append(dbt.length(column_name) ~ ' != ' ~ exact) %}{% endif %}
{%- if min is not none %}{% do conditions.append(dbt.length(column_name) ~ ' < ' ~ min) %}{% endif %}
{%- if max is not none %}{% do conditions.append(dbt.length(column_name) ~ ' > ' ~ max) %}{% endif %}
{%- if conditions | length == 0 %}
  {{ exceptions.raise_compiler_error("plexuspact.length needs `exact`, `min` or `max`") }}
{%- endif %}
select {{ column_name }} as failing_value
from {{ model }}
where {{ column_name }} is not null
  and ({{ conditions | join(' or ') }})
{% endtest %}
