{% test min(model, column_name, value) %}
select {{ column_name }} as failing_value
from {{ model }}
where {{ column_name }} is not null
  and {{ column_name }} < {{ value }}
{% endtest %}
