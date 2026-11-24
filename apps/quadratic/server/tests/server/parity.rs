use quadratic_server::app::{CATALOG_JSON, application};

#[test]
fn the_application_renders_the_catalog_file_byte_for_byte() {
    let app = application().expect("the application conforms to AcidCat");
    assert_eq!(app.render_catalog(), CATALOG_JSON);
}
