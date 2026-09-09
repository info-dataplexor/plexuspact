{#- A contract `assert` is an aggregate SQL predicate over the whole table
    (`MIN(amount) >= 0`, `COUNT(DISTINCT region) <= 12`); the test fails
    when it does not hold. -#}
{% test assert_expr(model, expression) %}
select 1 as assertion_failed
from {{ model }}
having not ({{ expression }})
{% endtest %}
