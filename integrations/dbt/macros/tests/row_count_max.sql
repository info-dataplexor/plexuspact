{% test row_count_max(model, count) %}
select count(*) as row_count
from {{ model }}
having count(*) > {{ count }}
{% endtest %}
