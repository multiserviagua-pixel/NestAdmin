-- «Redactar respuesta» (D159): como mucho una tarea viva por correo de origen. La expresión
-- protege contra una entrada que no sea JSON (las tareas antiguas y las de prueba pueden no
-- serlo): sin `redraft_of` no entra en el índice. Una tarea cancelada deja de ocupar el sitio.
CREATE UNIQUE INDEX tasks_redraft_of
    ON tasks ((CASE WHEN json_valid(input_ref) THEN json_extract(input_ref, '$.redraft_of') END))
    WHERE state <> 'cancelled'
      AND (CASE WHEN json_valid(input_ref) THEN json_extract(input_ref, '$.redraft_of') END) IS NOT NULL;
