import org.testcontainers.containers.GenericContainer;
import org.testcontainers.utility.DockerImageName;

public class Smoke {
    public static void main(String[] args) throws Exception {
        var image = DockerImageName.parse(System.getenv("MARSA_SAMPLE_IMAGE"));
        try (var container = new GenericContainer<>(image).withCommand("sleep", "180")) {
            container.start();
            var result = container.execInContainer("echo", "marsa-java");
            if (result.getExitCode() != 0 || !result.getStdout().strip().equals("marsa-java")) {
                throw new AssertionError("Unexpected exec result: " + result);
            }
        }
    }
}
