{% test row_count_min(model, count) %}
select count(*) as row_count
from {{ model }}
having count(*) < {{ count }}
{% endtest %}
