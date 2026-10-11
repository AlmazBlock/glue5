// The iOS executable's entry point: everything is in the glue5 library.
int glue5_main(int argc, char *argv[]);

int main(int argc, char *argv[]) {
    return glue5_main(argc, argv);
}
